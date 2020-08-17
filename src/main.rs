#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(unused_variables)]

mod sha_state;

use std::thread::spawn;
use std::path::{PathBuf, Path};
use crossbeam_channel::{Sender, Receiver};
use anyhow::{anyhow, Context, Result};
use log::{debug, error, info, trace, warn};
use std::fs::{symlink_metadata, FileType};
use sha1::{Sha1, Digest};
use std::fs;

mod cli;
mod worker_queue;

use worker_queue::WorkerQueue;
use std::io::Read;
use std::time::{Duration, Instant};
use crate::sha_state::{ShaState, ShaSet, DiffResult};
use std::sync::{Arc, RwLock, Mutex};
use crate::cli::Cli;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

pub struct Stats {
    pub fc: AtomicUsize,
    pub bc: AtomicUsize,
}

use lazy_static::lazy_static;

lazy_static! {
    pub static ref stats: Stats = Stats{ fc: AtomicUsize::new(0), bc: AtomicUsize::new(0) };
}


fn main() {
    if let Err(err) = sha_them_all() {
        eprintln!("ERROR in main: {}, {:#?}",&err, &err);
        std::process::exit(1);
    }
}

fn read_dir_thread(queue: &mut WorkerQueue<Option<PathBuf>>, out_q: &mut Sender<Option<PathBuf>>) {
    loop {
        match _read_dir_thread(queue, out_q) {
            Err(e) => {
                error!("read_dir thread top: {}", e);
            }
            Ok(()) => break,
        }
    }
}

fn _read_dir_thread(queue: &mut WorkerQueue<Option<PathBuf>>, send: &mut Sender<Option<PathBuf>>) -> Result<()> {
    loop {
        match queue.pop() {
            None => return Ok(()),
            Some(path) => {
                trace!("scanning dir {}", path.display());
                let dir_itr = match std::fs::read_dir(&path) {
                    Err(e) => {
                        error!("stat of dir: '{}', error: {}", path.display(), e);
                        continue;
                    }
                    Ok(rd) => rd,
                };
                for entry in dir_itr {
                    let entry = entry?;
                    let path = entry.path();
                    let md = match symlink_metadata(&entry.path()) {
                        Err(e) => {
                            error!("stat of file for symlink: '{}', error: {}", path.display(), e);
                            continue;
                        }
                        Ok(md) => md,
                    };

                    let file_type: FileType = md.file_type();
                    if !file_type.is_symlink() {
                        if file_type.is_file() {
                            trace!("sending file {}", path.display());
                            send.send(Some(path))?;
                        } else if file_type.is_dir() {
                            queue.push(Some(path))?;
                        }
                    }
                }
            }
        }
    }
}

fn sha_files(recv: &Receiver<Option<PathBuf>>, send: &Sender<Option<ShaState>>) -> usize {
    let mut size = 0;
    loop {
        match _sha_files(recv, send) {
            Err(e) => {
                error!("sha_file thread top: {}", e);
            }
            Ok(s) => {
                size += s;
                break
            },
        }
    }
    size
}

fn _sha_files(recv: &Receiver<Option<PathBuf>>, send: &Sender<Option<ShaState>>) -> Result<usize> {
    let mut buf = vec![0u8; 64 * 1024 * 1024];
    let mut size = 0;
    loop {
        trace!("waiting...");
        match recv.recv()? {
            None => return Ok(size), // this is the end my friend
            Some(path) => {
                match sha_a_file(&path, &mut buf) {
                    Err(e) => {
                        error!("sha1 on file {} failed, {}", &path.display(), e);
                        ()
                    }
                    Ok( (state,sz)) => {
                        size += sz;
                        stats.bc.fetch_add(sz, Ordering::Relaxed);
                        stats.fc.fetch_add(1, Ordering::Relaxed);
                        send.send(Some(state))?;
                        ()
                    }
                }
            }
        };
    }
}

fn sha_a_file(path: &Path, buf: &mut Vec<u8>) -> Result<(ShaState, usize)> {
    let mut file = fs::File::open(&path).context("open failed")?;
    let (hash,size) = sha1_digest(&mut file, buf).context("digest_reader failed")?;
    trace!("path: \"{}\" sha1: {}", path.display(), &hash.to_string());
    let mtime = path.metadata()?.modified()?;
    Ok((ShaState::new(path.to_path_buf(), hash, mtime), size))
}

fn sha1_digest<R: Read>(mut reader: R, buf: &mut Vec<u8>) -> Result<(Digest, usize)> {
    let mut m = sha1::Sha1::new();
    let mut size = 0;
    loop {
        let count = reader.read(&mut buf[..])?;
        size += count;
        if count == 0 {
            break;
        }
        m.update(&buf[..count]);
    }

    Ok((m.digest(), size))
}

fn record_state(cli: &Arc<Cli>, recv: Receiver<Option<ShaState>>, state: &mut Arc<Mutex<ShaSet>>) {
    loop {
        match recv.recv() {
            Err(e) => panic!("write thread errored during receive: {}", e),
            Ok(None) => return,
            Ok(Some(state_entry)) => {
                match state.lock() {
                    Err(e) => panic!("write thread error locking state {}", e),
                    Ok(mut state) => {
                        let info = state_entry.to_string();
                        match state.add(state_entry) {
                            Err(e) => error!("Cannot add entry for {} due to {}", info, e),
                            Ok(diff) => {
                                match diff {
                                    DiffResult::BothDiff => warn!("SHA TIME CHANGE: {}", info),
                                    DiffResult::ShaDiff => warn!("SHA CHANGE: {}", info),
                                    DiffResult::TimeDiff => warn!("TIME CHANGE: {}", info),
                                    _ => (),
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn ticker() {
    let mut bc = 0;
    let mut fc = 0;
    loop {
        std::thread::sleep(Duration::from_secs(1));
        bc = stats.bc.load(Ordering::Relaxed);
        fc = stats.fc.load(Ordering::Relaxed);
        info!("TICK  {} files  {} GB", fc, bc/(1024*1024*1024));
    }
}

fn sha_them_all() -> Result<()> {

    let cli = Arc::new(crate::cli::get_cli());

    stderrlog::new()
        .module(module_path!())
        .quiet(false)
        .verbosity(cli.verbosity)
        .timestamp(stderrlog::Timestamp::Millisecond) // opt.ts.unwrap_or(stderrlog::Timestamp::Off))
        .init()
        .unwrap();

    let h_ticker = spawn(move || ticker());

    let mut dir_q: WorkerQueue<Option<PathBuf>> = WorkerQueue::new(cli.threads_dir, 0);
    let (send, recv) = crossbeam_channel::unbounded();
    let (send_state, recv_state) = crossbeam_channel::unbounded();

    let mut state = Arc::new(Mutex::new(ShaSet::new(&cli.state_path)?));

    let start = Instant::now();

    let h_state_write = {
        let cli_c = cli.clone();
        let mut state_c = state.clone();
        spawn(move || record_state(&cli_c, recv_state, &mut state_c))
    };

    let mut h_dir_threads = vec![];
    for _i in 0..cli.threads_dir {
        let mut dir_q = dir_q.clone();
        let mut send = send.clone();
        let h = spawn(move || read_dir_thread(&mut dir_q, &mut send));
        h_dir_threads.push(h);
    }

    let mut h_sha_threads = vec![];
    for _i in 0..cli.threads_sha {
        let mut recv = recv.clone();
        let mut send_state = send_state.clone();
        let h = spawn(move || sha_files(&mut recv, &mut send_state));
        h_sha_threads.push(h);
    }

    // prime the read dir pump
    dir_q.push(Some(cli.top_dir.clone()));


    // wait on work as boss queue - then stop them
    loop {
        let x = dir_q.wait_for_finish_timeout(Duration::from_millis(250))?;
        if x != -1 { break; }
    }
    for _ in 0..cli.threads_dir { dir_q.push(None)?; }
    for h in h_dir_threads {
        h.join().unwrap();
    }
    info!("directory scanning is done");

    // wait on sha threads
    for _ in 0..cli.threads_sha { send.send(None)?; }
    let mut tot_bytes = 0;
    for h in h_sha_threads {
        tot_bytes += h.join().unwrap();
    }
    let secs = start.elapsed().as_secs_f64();
    let rate = (tot_bytes as f64 / secs)/(1024.0*1024.0);
    info!("sha of files is done in {:.3} secs {} total  {:.2}MB/ sec", secs, tot_bytes, rate);

    send_state.send(None)?;
    h_state_write.join().unwrap();

    match state.lock() { // this match is needed I think because LockGuard points to special version of Result
        Err(e) => panic!("cannot lock state at the to write the current entries"),
        Ok(mut s) => s.write_entries(&cli.state_path)?,
    }

    Ok(())
}
