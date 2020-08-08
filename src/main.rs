#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(unused_variables)]

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

fn sha_files(recv: &Receiver<Option<PathBuf>>) {
    loop {
        match _sha_files(recv) {
            Err(e) => {
                error!("sha_file thread top: {}", e);
            }
            Ok(()) => break,
        }
    }

}

fn _sha_files(recv: &Receiver<Option<PathBuf>>) -> Result<()> {
    loop {
        trace!("waiting...");
        match recv.recv()? {
            None => return Ok(()), // this is the end my friend
            Some(path) => {
                match sha_a_file(&path) {
                    Err(e) => error!("sha1 on file fail"),
                    Ok(()) => (),
                }
            },
        };
    }
}

fn sha_a_file(path: &Path) -> Result<()> {
    let mut file = fs::File::open(&path).context("open failed")?;
    let hash = sha1_digest(&mut file).context("digest_reader failed")?;
    info!("path: \"{}\" sha1: {}", path.display(), &hash.to_string());
    Ok(())
}
fn sha1_digest<R: Read>(mut reader: R) -> Result<Digest> {
    trace!("doing sha1 on file");
    let mut m = sha1::Sha1::new();
    let mut buffer = [0; 1024*1024];

    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        m.update(&buffer[..count]);
    }

    Ok(m.digest())
}

fn sha_them_all() -> Result<()>{
    println!("Hello, world!");
    let cli = crate::cli::get_cli();

    stderrlog::new()
        .module(module_path!())
        .quiet(false)
        .verbosity(cli.verbosity)
        .timestamp(stderrlog::Timestamp::Millisecond) // opt.ts.unwrap_or(stderrlog::Timestamp::Off))
        .init()
        .unwrap();

    let mut dir_q: WorkerQueue<Option<PathBuf>> = WorkerQueue::new(cli.dir_threads, 0);
    let (send,recv) = crossbeam_channel::unbounded();

    let mut h_dir_threads = vec![];
    for _i in 0..cli.dir_threads {
        let mut dir_q = dir_q.clone();
        let mut send = send.clone();
        let h = spawn(move || read_dir_thread(&mut dir_q, &mut send));
        h_dir_threads.push(h);
    }

    let mut h_sha_threads = vec![];
    for _i in 0..cli.sha_threads {
        let mut recv = recv.clone();
        let h = spawn(move || sha_files(&mut recv));
        h_sha_threads.push(h);
    }

    // prime the read dir pump
    dir_q.push(Some(cli.top_dir.clone()));


    // wait on work as boss queue - then stop them
    loop {
        let x = dir_q.wait_for_finish_timeout(Duration::from_millis(250))?;
        if x != -1 { break; }
    }
    for _ in 0..cli.dir_threads { dir_q.push(None)?; }
    for h in h_dir_threads {
        h.join().unwrap();
    }
    info!("directory scanning is done");

    // wait on sha threads
    for _ in 0..cli.sha_threads { send.send(None)?; }
    for h in h_sha_threads {
        h.join().unwrap();
    }
    info!("sha of files is done");
    Ok(())



}
