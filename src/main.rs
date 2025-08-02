use std::{
    collections::HashMap,
    ffi::OsString,
    fs::File,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use chrono::{Datelike, NaiveDate, NaiveDateTime};
use clap::Parser;
use itertools::Itertools;
use serde::Deserialize;
use walkdir::WalkDir;

#[derive(thiserror::Error, Debug, Clone)]
pub enum OrganizerError {
    #[error("Can't parse path: {0:?}")]
    WrongPathBuf(PathBuf),
}

#[derive(Parser)]
struct Cli {
    source: PathBuf,
    #[arg(long, short)]
    target: Option<PathBuf>,
    #[arg(long, short, action)]
    dry_run: bool,
}

#[derive(Deserialize, Clone)]
#[serde(tag = "_type")]
pub enum InfoJson {
    #[serde(rename = "video")]
    Video(VideoJson),
    #[serde(rename = "playlist")]
    Playlist,
}

#[derive(Deserialize, Clone)]
pub struct VideoJson {
    pub id: String,
    pub title: String,
    pub channel: String,
    pub fulltitle: String,
    pub upload_date: String,
    pub timestamp: Option<i64>,
    pub playlist_webpage_url: Option<String>,
}

impl VideoJson {
    pub fn get_date(&self) -> anyhow::Result<NaiveDateTime> {
        if let Some(timestamp) = self.timestamp {
            Ok(NaiveDateTime::from_timestamp(timestamp, 0))
        } else {
            let date = NaiveDate::parse_from_str(&self.upload_date, "%Y%m%d")?;
            Ok(date.into())
        }
    }

    pub fn is_short(&self) -> bool {
        self.playlist_webpage_url
            .as_ref()
            .map_or_else(|| false, |s| s.ends_with("/shorts"))
    }
}

#[derive(Clone)]
struct CatalogueEntry {
    pub date: NaiveDateTime,
    pub json: VideoJson,
    pub path: Vec<PathBuf>,
}

impl CatalogueEntry {
    pub fn get_date(&self) -> NaiveDateTime {
        self.date.clone()
    }

    pub fn get_title(&self) -> String {
        if self.json.fulltitle.len() > 1 {
            self.json.fulltitle.clone()
        } else {
            self.json.title.clone()
        }
    }
}

impl CatalogueEntry {
    pub fn new(path: &Path) -> anyhow::Result<Option<Self>> {
        let json: InfoJson = serde_json::from_reader(File::open(path)?)?;

        match json {
            InfoJson::Video(video_json) => {
                if video_json.is_short() {
                    Ok(None)
                } else {
                    Ok(Some(CatalogueEntry {
                        date: video_json.get_date()?,
                        json: video_json,
                        path: CatalogueEntry::get_other_files(path)?,
                    }))
                }
            }
            InfoJson::Playlist => Ok(None),
        }
    }

    fn get_other_files(path: &Path) -> anyhow::Result<Vec<PathBuf>> {
        let file_name = path.file_name().unwrap().to_str().unwrap();
        let info_ext = ".info.json";
        let ext_len = info_ext.len();

        if !file_name.ends_with(info_ext) {
            return Ok(Vec::new());
        }

        let file_name = &file_name[0..file_name.len() - ext_len];

        let dirname = match path.parent() {
            Some(dirname) => dirname,
            None => Path::new("."),
        };

        let mut r = Vec::new();
        r.push(PathBuf::from(path));
        for e in std::fs::read_dir(dirname)?.flatten() {
            if e.file_type()?.is_file() {
                let other_name = match e.path().file_stem().unwrap().to_os_string().into_string() {
                    Ok(name) => name,
                    Err(_) => continue,
                };

                if other_name == file_name {
                    r.push(e.path().clone());
                }
            }
        }

        Ok(r)
    }
}

pub struct VideoCatalogue {
    raw: Vec<CatalogueEntry>,
}

impl VideoCatalogue {
    pub fn build(source: PathBuf) -> anyhow::Result<Self> {
        let mut cat = Vec::new();

        let iter = WalkDir::new(source)
            .into_iter()
            .filter_map(|e| if let Ok(entry) = e { Some(entry) } else { None })
            .filter(|e| {
                if let Some(ext) = e.path().extension() {
                    ext == "json"
                } else {
                    false
                }
            });

        for e in iter {
            println!("Parsing {:?}", e.file_name());
            let entry = CatalogueEntry::new(e.path())?;
            if let Some(video) = entry {
                cat.push(video);
            }
        }

        Ok(Self { raw: cat })
    }

    fn by_channel(&self) -> HashMap<String, Vec<&CatalogueEntry>> {
        self.raw
            .iter()
            .into_group_map_by(|e| e.json.channel.clone())
    }

    pub fn build_seasons<'a>(&'a self) -> Vec<SeasonedStructure<'a>> {
        let mut r = Vec::new();
        let chans = self.by_channel();
        for (c, vids) in chans {
            r.push(VideoCatalogue::build_channel(&c, vids));
        }

        r
    }

    fn build_channel<'a>(name: &str, mut vids: Vec<&'a CatalogueEntry>) -> SeasonedStructure<'a> {
        let mut seasons = Vec::new();

        vids.sort_by_key(|a| a.date);
        for (index, (year, vids)) in vids
            .iter()
            .chunk_by(|v| v.date.year())
            .into_iter()
            .enumerate()
        {
            let mut videos_in_season = Vec::new();
            for v in vids {
                videos_in_season.push(*v);
            }

            seasons.push(Season {
                number: index + 1,
                videos: videos_in_season,
            });
        }

        SeasonedStructure {
            channel_name: name.to_string(),
            seasons,
        }
    }
}

pub struct Season<'a> {
    pub number: usize,
    pub videos: Vec<&'a CatalogueEntry>,
}

impl<'a> Season<'a> {
    fn print(&self) {
        for (ep, v) in self.videos.iter().enumerate() {
            println!(
                " S{:0>3}E{ep:0>3}: {} ({})",
                self.number,
                v.get_title(),
                v.get_date()
            );
        }
    }
}

pub struct SeasonedStructure<'a> {
    pub channel_name: String,
    pub seasons: Vec<Season<'a>>,
}

impl<'a> SeasonedStructure<'a> {
    fn print(&self) {
        println!("Channel: {}", self.channel_name);
        for s in &self.seasons {
            s.print();
        }
    }
}

pub struct DirectoryBuilder<'a> {
    channel: SeasonedStructure<'a>,
    base: PathBuf,
    dry_run: bool,
    verbose: bool,
}

impl<'a> DirectoryBuilder<'a> {
    pub fn new(base_path: &PathBuf, channel: SeasonedStructure<'a>, dry_run: bool) -> Self {
        let mut base = base_path.clone();
        base.push(channel.channel_name.clone());
        Self {
            channel,
            base,
            dry_run,
            verbose: true,
        }
    }

    pub fn build(&self) -> anyhow::Result<()> {
        self.create_channel_directory()?;

        for season in &self.channel.seasons {
            let season_dir = self.create_season_directory(&season)?;

            for (ep, vid) in season.videos.iter().enumerate() {
                self.link_video_data(&season_dir, ep + 1, &vid)?;
            }
        }

        Ok(())
    }

    fn link_video_data(
        &self,
        season_dir: &PathBuf,
        ep_no: usize,
        entry: &'a CatalogueEntry,
    ) -> anyhow::Result<()> {
        let base_file_name = format!("{}", entry.get_title().replace("/", "_"));

        for file in entry.path.iter() {
            let mut base_file_name = OsString::from(base_file_name.clone());
            let ext: OsString = file.extension().unwrap().into();

            base_file_name.push(".");
            base_file_name.push(ext);

            let mut target = season_dir.clone();
            target.push(base_file_name);

            let target = PathBuf::from(target);
            self.create_symlink(file, &target)?;
        }

        Ok(())
    }

    fn create_symlink(&self, source: &PathBuf, target: &PathBuf) -> anyhow::Result<()> {
        if self.dry_run || self.verbose {
            println!("Linking: {source:?} -> {target:?}");

            if self.dry_run {
                return Ok(());
            }
        }

        match std::os::unix::fs::symlink(source, target) {
            Ok(_) => {}
            Err(err) => {
                if err.kind() != ErrorKind::AlreadyExists {
                    Err(err)?;
                } else {
                    return Ok(());
                }
            }
        }

        Ok(())
    }

    fn create_season_directory(&self, season: &Season<'a>) -> anyhow::Result<PathBuf> {
        let season_dir = {
            let mut d = self.base.clone();
            d.push(format!("Season {}", season.number));

            d
        };

        if self.dry_run || self.verbose {
            println!("Creating directory: {:?}", season_dir);

            if self.dry_run {
                return Ok(season_dir);
            }
        }

        std::fs::create_dir_all(&season_dir)?;

        Ok(season_dir)
    }

    fn create_channel_directory(&self) -> anyhow::Result<()> {
        if self.dry_run || self.verbose {
            println!("Creating directory: {:?}", self.base);

            if self.dry_run {
                return Ok(());
            }
        }

        std::fs::create_dir_all(&self.base)?;

        Ok(())
    }
}

fn main() -> Result<(), anyhow::Error> {
    let cli = Cli::parse();

    let cat = VideoCatalogue::build(cli.source)?;
    let structure = cat.build_seasons();

    if let Some(target) = cli.target {
        for chan in structure {
            DirectoryBuilder::new(&target, chan, cli.dry_run).build()?;
        }
    }

    Ok(())
}
