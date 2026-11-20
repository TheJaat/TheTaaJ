//! tfstool - make, inspect and populate a TaajFS image from the host.
//!
//! The Rust rewrite of tools/tfs/tfstool.c. The C tool stays as the
//! reference implementation, and `crosscheck.sh` checks that each tool
//! reads what the other writes - and that both write identical bytes.
//!
//!   tfstool mkfs  <image> [blocks]
//!   tfstool dump  <image>
//!   tfstool ls    <image> [dir]
//!   tfstool mkdir <image> <dir>               in a directory that exists
//!   tfstool put   <image> <name> <file>       create or replace, one commit
//!   tfstool get   <image> <name>              contents to stdout
//!   tfstool rm    <image> <name>
//!   tfstool touch <image> <name>              an empty file
//!   tfstool write <image> <name> <offset> <file>
//!
//! A name is a path from the root: `docs/notes`. The C tool knows no
//! directories; a volume that has had one made on it is marked (format
//! version 2), and the C tool then refuses it.

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::process::ExitCode;

use tfstool::device::FileDevice;
use tfstool::dump::dump;
use tfstool::format::{BLOCK_SIZE, TYPE_DIR};
use tfstool::volume::{self, Volume};

const USAGE: &str = "\
usage: tfstool mkfs  <image> [blocks]
       tfstool dump  <image>
       tfstool ls    <image> [dir]
       tfstool mkdir <image> <dir>
       tfstool put   <image> <name> <file>
       tfstool get   <image> <name>
       tfstool rm    <image> <name>
       tfstool mv    <image> <from> <to>
       tfstool touch <image> <name>
       tfstool write <image> <name> <offset> <file>";

fn main() -> ExitCode {
    let owned: Vec<String> = env::args().skip(1).collect();
    let args: Vec<&str> = owned.iter().map(String::as_str).collect();

    // Every failure comes back as a message. Printing happens in exactly
    // one place, and every failure exits non-zero - which the C tool
    // does not always do.
    match run(&args) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

/// Dispatches on the command line using *slice patterns*: each arm
/// matches the exact shape of the arguments and names the pieces. A
/// wrong number of arguments simply matches no arm and gets the usage.
fn run(args: &[&str]) -> Result<ExitCode, String> {
    match args {
        ["mkfs", image] => mkfs(image, 4096),
        ["mkfs", image, blocks] => {
            let blocks = blocks
                .parse::<u32>()
                .map_err(|_| format!("mkfs: '{blocks}' is not a block count"))?;
            mkfs(image, blocks)
        }

        ["dump", image] => {
            let file = File::open(image).map_err(|e| format!("cannot open {image}: {e}"))?;
            let healthy = dump(&mut FileDevice::new(file), &mut io::stdout().lock())
                .map_err(|e| format!("dump: {e}"))?;
            Ok(if healthy {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }

        ["ls", image] => ls(image, ""),
        ["ls", image, directory] => ls(image, directory),

        ["mkdir", image, name] => {
            open(image)?
                .make_directory(name)
                .map_err(|e| format!("mkdir: {name}: {e}"))?;
            println!("mkdir: ok");
            Ok(ExitCode::SUCCESS)
        }

        ["put", image, name, source] => {
            let data = fs::read(source).map_err(|e| format!("cannot read {source}: {e}"))?;
            let mut v = open(image)?;
            v.put(name, &data).map_err(|e| format!("put: {e}"))?;
            println!("put: {} bytes -> {name} (ok)", data.len());
            Ok(ExitCode::SUCCESS)
        }

        ["get", image, name] => {
            let mut v = open(image)?;
            let file = v.lookup(name).map_err(|e| format!("get: {name}: {e}"))?;
            let data = v.read_all(&file).map_err(|e| format!("get: {e}"))?;
            io::stdout()
                .lock()
                .write_all(&data)
                .map_err(|e| format!("get: {e}"))?;
            Ok(ExitCode::SUCCESS)
        }

        ["rm", image, name] => {
            open(image)?
                .delete(name)
                .map_err(|e| format!("rm: {name}: {e}"))?;
            println!("rm: ok");
            Ok(ExitCode::SUCCESS)
        }

        ["mv", image, from, to] => {
            open(image)?
                .rename(from, to)
                .map_err(|e| format!("mv: {from} to {to}: {e}"))?;
            println!("mv: ok");
            Ok(ExitCode::SUCCESS)
        }

        ["touch", image, name] => {
            open(image)?
                .create(name)
                .map_err(|e| format!("touch: {name}: {e}"))?;
            println!("touch: ok");
            Ok(ExitCode::SUCCESS)
        }

        ["write", image, name, offset, source] => {
            let offset = offset
                .parse::<u32>()
                .map_err(|_| format!("write: '{offset}' is not an offset"))?;
            let data = fs::read(source).map_err(|e| format!("cannot read {source}: {e}"))?;
            let written = open(image)?
                .write_at(name, offset, &data)
                .map_err(|e| format!("write: {name}: {e}"))?;
            println!("write: {written} bytes at {offset} -> {name} (ok)");
            Ok(ExitCode::SUCCESS)
        }

        _ => Err(USAGE.to_string()),
    }
}

/// What is in one directory. Without directories in it, the output is
/// what it always was - and what the C tool prints.
fn ls(image: &str, directory: &str) -> Result<ExitCode, String> {
    let mut v = open(image)?;
    let entries = v.list_in(directory).map_err(|e| format!("ls: {directory}: {e}"))?;
    let mut directories = 0;
    for entry in &entries {
        if entry.kind == TYPE_DIR {
            println!("  {:<24} {:>8}", format!("{}/", entry.name), "dir");
            directories += 1;
        } else {
            println!("  {:<24} {:>8}", entry.name, entry.size);
        }
    }
    let files = entries.len() - directories;
    if directories == 0 {
        println!("  {files} files, {} blocks free", v.free_blocks());
    } else {
        println!("  {directories} directories, {files} files, {} blocks free", v.free_blocks());
    }
    Ok(ExitCode::SUCCESS)
}

fn mkfs(image: &str, blocks: u32) -> Result<ExitCode, String> {
    let file = File::create(image).map_err(|e| format!("mkfs: cannot create {image}: {e}"))?;
    let mut device = FileDevice::new(file);

    // `format` checks the size before writing anything.
    volume::format(&mut device, blocks).map_err(|e| format!("mkfs: {e}"))?;

    // Size the image to the full volume. Unlike the C tool, which writes
    // every zero block, `set_len` makes a sparse file: it reads as zeros
    // but takes no disk space until a block is actually written.
    let file = File::options()
        .write(true)
        .open(image)
        .map_err(|e| format!("mkfs: {e}"))?;
    file.set_len(u64::from(blocks) * BLOCK_SIZE as u64)
        .map_err(|e| format!("mkfs: {e}"))?;

    println!("mkfs: ok, {blocks} blocks of {BLOCK_SIZE} bytes -> {image}");
    Ok(ExitCode::SUCCESS)
}

fn open(image: &str) -> Result<Volume<FileDevice>, String> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(image)
        .map_err(|e| format!("cannot open {image}: {e}"))?;
    Volume::mount(FileDevice::new(file)).map_err(|e| format!("mount: {e}"))
}
