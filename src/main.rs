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
use std::time::Duration;
use crate::sha_state::{ShaState, ShaSet, DiffResult};
use std::sync::{Arc, RwLock, Mutex};
use crate::cli::Cli;

fn main() {
    if let Err(err) = sha_them_all() {
        eprintln!("ERROR in main: {}", &err);
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
                debug!("scanning dir {}", path.display());
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

fn sha_files(recv: &Receiver<Option<PathBuf>>, send: &Sender<Option<ShaState>>) {
    loop {
        match _sha_files(recv, send) {
            Err(e) => {
                error!("sha_file thread top: {}", e);
            }
            Ok(()) => break,
        }
    }

}

fn _sha_files(recv: &Receiver<Option<PathBuf>>, send: &Sender<Option<ShaState>>) -> Result<()> {
    let mut buf = vec![0u8; 64*1024*1024];
    loop {
        trace!("waiting...");
        match recv.recv()? {
            None => return Ok(()), // this is the end my friend
            Some(path) => {
                match sha_a_file(&path, &mut buf) {
                    Err(e) => {error!("sha1 on file fail"); ()},
                    Ok(state) => {send.send(Some(state))?; ()},
                }
            },
        };
    }
}

fn sha_a_file(path: &Path, buf: &mut Vec<u8>) -> Result<ShaState> {
    let mut file = fs::File::open(&path).context("open failed")?;
    let hash = sha1_digest(&mut file, buf).context("digest_reader failed")?;
    info!("path: \"{}\" sha1: {}", path.display(), &hash.to_string());
    let mtime = path.metadata()?.modified()?;
    Ok(ShaState::new(path.to_path_buf(), hash, mtime))
}
fn sha1_digest<R: Read>(mut reader: R, buf: &mut Vec<u8>) -> Result<Digest> {
    trace!("doing sha1 on file");
    let mut m = sha1::Sha1::new();
    //let mut buffer = [0; 1024*1024];

    loop {
        let count = reader.read(&mut buf[..])?;
        if count == 0 {
            break;
        }
        m.update(&buf[..count]);
    }

    Ok(m.digest())
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

fn sha_them_all() -> Result<()>{
    println!("Hello, world!");
    let cli = Arc::new(crate::cli::get_cli());

    stderrlog::new()
        .module(module_path!())
        .quiet(false)
        .verbosity(cli.verbosity)
        .timestamp(stderrlog::Timestamp::Millisecond) // opt.ts.unwrap_or(stderrlog::Timestamp::Off))
        .init()
        .unwrap();

    let mut dir_q: WorkerQueue<Option<PathBuf>> = WorkerQueue::new(cli.threads_dir, 0);
    let (send,recv) = crossbeam_channel::unbounded();
    let (send_state,recv_state) = crossbeam_channel::unbounded();

    let mut state = Arc::new(Mutex::new(ShaSet::new(&cli.state_path)?));

    let h_state_write = {
        let cli_c = cli.clone();
        let mut state_c = state.clone();
        spawn(move|| record_state(&cli_c, recv_state, &mut state_c))
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
    for h in h_sha_threads {
        h.join().unwrap();
    }

    send_state.send(None)?;
    h_state_write.join().unwrap();

    {
        match state.lock() {
            Err(e) => panic!("cannot lock state at the to write the current entries"),
            Ok(mut s) => s.write_entries(&cli.state_path)?,
        }
    }
    //lock.write_entries(&cli.state_path);

    info!("sha of files is done");
    Ok(())
}
