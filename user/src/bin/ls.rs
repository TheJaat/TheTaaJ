//! ls: lists the files - a program on the disk, not part of the shell.
//!
//! It can, because whoever starts it hands it a channel to the file
//! service (taaj::file_service). Without one it has no files to list:
//! there is no global file system a program can reach on its own.

#![no_std]
#![no_main]

use taaj::println;

taaj::program!(main);

fn main() -> i64 {
    let Some(files) = taaj::file_service() else {
        println!("ls: started without the file service");
        return 2;
    };
    let mut bytes = 0;
    let listed = files.list(|name, size| {
        println!("{size:>8}  {name}");
        bytes += size;
    });
    match listed {
        Ok(0) => println!("no files"),
        Ok(count) => println!("{count} file{}, {bytes} bytes", if count == 1 { "" } else { "s" }),
        Err(error) => {
            println!("ls: {}", error.describe());
            return 1;
        }
    }
    0
}
