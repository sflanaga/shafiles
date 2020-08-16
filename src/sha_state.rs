#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(unused_variables)]


use std::path::{PathBuf, Path};
use sha1::Digest;
use anyhow::{bail, anyhow, Context, Result};
use log::{debug, error, info, trace, warn};
use std::sync::{Arc, RwLock};
use std::collections::BTreeSet;
use std::time::{SystemTime, Duration, Instant};
use std::fs::File;
use std::io::{BufRead, BufWriter, Write, BufReader};
use std::str::FromStr;
use std::cmp::Ordering;
use std::ops::Add;
use serde::{ser, de, Serialize, Deserialize};


#[derive(Debug, Eq, Clone, Serialize, Deserialize)]
pub struct ShaState {
    path: PathBuf,
    sha: Digest,
    mtime: SystemTime,
    t_deltas: u64,
    sha_deltas: u64,
}

impl PartialEq for ShaState {
    fn eq(&self, other: &Self) -> bool {
        self.path.cmp(&other.path) == Ordering::Equal
    }
}

impl PartialOrd for ShaState {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.path.cmp(&other.path))
    }
}

impl Ord for ShaState {
    fn cmp(&self, other: &Self) -> Ordering {
        self.path.cmp(&other.path)
    }
}

trait ToErr<T> {
    fn to_err(self: Self) -> Result<T>;
}

impl<T> ToErr<T> for Option<T> {
    fn to_err(self) -> Result<T> {
        match self {
            None => bail!("got none when something was expect"),
            Some(v) => Ok(v),
        }
    }
}

fn digest_from_str(dig_str: &str) -> Result<Digest> {
    match Digest::from_str(&dig_str) {
        Err(_) => return Err(anyhow!("unable to convert string to sha1 digest str: \"{}\"", dig_str)),
        Ok(v) => Ok(v),
    }
}

/*
type ResultSer<T,E> = std::result::Result<T,E>;
impl Serialize for Digest {
    fn serialize<S>(&self, serializer: S) -> ResultSer<<S as Serializer>::Ok, <S as Serializer>::Error> where
        S: Serializer {
        serializer.serialize_str(self.to_string()?)?
    }
}

impl Deserialize for Digest {
    fn deserialize<D>(deserializer: D) -> ResultSer<Self, <D as Deserializer<'de>>::Error> where
        D: Deserializer<'de> {
        unimplemented!()
    }
}
*/

impl ShaState {
    pub fn new(path: PathBuf, sha: Digest, mtime: SystemTime) -> Self {
        ShaState { path: path, sha: sha, mtime: mtime, t_deltas: 0, sha_deltas: 0 }
    }

    fn from_str(s: &str) -> Result<Self> {
        fn inner(s: &str) -> Result<ShaState> {
            let mut v = s.split('\0');
            let path = PathBuf::from(v.next().to_err()?);
            let sha = digest_from_str(v.next().to_err()?)?;

            let mtime_p = v.next().to_err()?.parse().context("cannot parse mtime number")?;
            let mtime = SystemTime::UNIX_EPOCH;
            mtime.add(Duration::from_secs(mtime_p));

            let t_deltas = v.next().to_err()?.parse().context("cannot parse time deltas number")?;
            let sha_deltas = v.next().to_err()?.parse().context("cannot parse sha deltas number")?;

            Ok(ShaState {
                path: path,
                sha: sha,
                mtime: mtime,
                t_deltas,
                sha_deltas,
            })
        }
        let r = inner(s).with_context(|| format!("trying to parse shastate line: \"{}\"", &s))?;
        Ok(r)
    }
    pub fn write(&self, f: &mut dyn Write) -> Result<()> {
        write!(f, "{}\0{}\0{}\0{}\0{}\n", self.path.display(), self.sha.to_string(), self.mtime.elapsed().unwrap().as_secs(), self.t_deltas, self.sha_deltas)?;
        Ok(())
    }

    pub fn to_string(&self) -> String {
        return format!("{}\0{}\0{}\0{}\0{}", self.path.display(), self.sha.to_string(), self.mtime.elapsed().unwrap().as_secs(), self.t_deltas, self.sha_deltas);
    }
}

#[derive(Debug, Clone)]
pub enum DiffResult {
    Added,
    BothDiff,
    ShaDiff,
    TimeDiff,
    Same,
}

#[derive(Serialize, Deserialize)]
pub struct ShaSet(BTreeSet<ShaState>);

impl ShaSet {
    pub(crate) fn new(path: &PathBuf) -> Result<Self> {
        let now = SystemTime::now();

        let f_h = match File::open(&path) {
            Err(e) => {
                warn!("There is no initial state file at \"{}\", so going with an initial empty one. {}", path.display(), e);
                return Ok(ShaSet(BTreeSet::new()));
            }
            Ok(f) => f,
        };
        let start = Instant::now();
        let buf = BufReader::new(f_h);
        let set: ShaSet = serde_json::from_reader(buf)?;
        info!("read state file: \"{}\" in {:.3} secs", path.display(), start.elapsed().as_secs_f64());
        // let mut de = serde_json::Deserializer::from_reader(&f_h);
        //
        // let set:ShaSet = ShaSet::deserialize(de)?;

        Ok(set)
    }

    pub fn add(&mut self, mut e: ShaState) -> Result<DiffResult> {
        let mut res = Ok(DiffResult::Added);
        match self.0.take(&e) {
            Some(mut v) => {
                match (v.sha == e.sha, v.mtime == e.mtime) {
                    (true, true) => res = Ok(DiffResult::Same),
                    (false, false) => {
                        e.sha_deltas = v.sha_deltas + 1;
                        e.t_deltas = v.t_deltas + 1;
                        res = Ok(DiffResult::BothDiff)
                    }
                    (true, false) => {
                        e.t_deltas = v.t_deltas + 1;
                        res = Ok(DiffResult::TimeDiff)
                    }
                    (false, true) => {
                        e.sha_deltas = v.sha_deltas + 1;
                        res = Ok(DiffResult::ShaDiff)
                    }
                }
                self.0.insert(e);
                return res;
            }
            None => {
                self.0.insert(e);
                return Ok(DiffResult::Added);
            }
        }
    }

    fn entries_from(path: &Path, set: &mut BTreeSet<ShaState>) -> Result<()> {
        let now = SystemTime::now();

        let f_h = match File::open(&path) {
            Err(e) => {
                warn!("There is no initial state file at \"{}\", so going with an initial empty one. {}", path.display(), e);
                return Ok(());
            }
            Ok(f) => f,
        };
        let lines = std::io::BufReader::new(f_h).lines();
        let mut count = 0;
        for l in lines {
            count += 1;
            let l = l.with_context(|| format!("unable parse data file:{}:{}", &path.display(), count))?;
            match ShaState::from_str(&l) {
                Err(e) => {
                    error!("skipping a line due to {:?}", e);
                    ()
                }
                Ok(t) => {
                    set.insert(t);
                    ()
                }
            }
        }
        Ok(())
    }

    pub fn write_entries(&self, path: &PathBuf) -> Result<()> {
        let start = Instant::now();

        let mut tmppath = path.clone();
        let mut filename = String::from(".tmp_");
        filename.push_str(path.file_name().unwrap().to_str().unwrap());
        tmppath.pop();
        tmppath.push(filename);
        { // this scope forces drop of file for renaming
            let file = File::create(&tmppath)
                .with_context(|| format!("Unable to create tmpfile: \"{}\" to write tracking data too", &tmppath.display()))?;
            let mut buf = BufWriter::new(&file);

            serde_json::to_writer_pretty(buf, &self)?;
            // for e in self.0.iter() {
            //     e.write(&mut buf)?;
            // }
        }
        std::fs::rename(&tmppath, &path)
            .with_context(|| format!("Unable to post rename tmp file after writing tracking information: rename \"{}\" to \"{}\"", &tmppath.display(), &path.display()))?;
        info!("wrote state file: {} in {:.3} secs", path.display(), start.elapsed().as_secs_f64());
        Ok(())
    }
}


