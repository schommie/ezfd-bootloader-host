use clap::Parser;
use std::path::{Path,PathBuf};
use std::fs;
#[derive(Parser, Debug)]
struct Args {
    #[arg(short, long)]
    binary: PathBuf,

    #[arg(short, long, default_value = "can0")]
    interface: String,

    #[arg(short, long)]
    node_id: String,
}

fn main() {
    let args = Args::parse();

    let resolved_path = if args.binary.exists() {
        args.binary
    } else {
        let fallback = Path::new("./binaries").join(&args.binary);
        if fallback.exists() {
            fallback
        } else {
            args.binary
        }
    };

    println!("Attempting to read: {:?}", resolved_path);
    let binvec: Vec<u8>;
    match fs::read(&resolved_path) {
        Ok(bytes) => {
            println!("Success! Read {} bytes.", bytes.len());
            binvec = bytes;
        }
        Err(e) => {
            eprintln!("Error reading binary file {:?}: {}", resolved_path, e);
            std::process::exit(1);
        }
    }

    for i in 0..10{
        println!("Byte {} = {}",i,hex::encode(&[binvec[i]]));
    }




    let nodeid: u32 = match u32::from_str_radix(&args.node_id, 16) {
        Ok(id) if id <= 0x1F => {
            println!("Attempting to connect to Node ID: 0x{:02X} ({})", id, id);
            id
        }
        Ok(_) => {
            eprintln!("Error: Node ID 0x{} is out of range (max 0x1F).", args.node_id);
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error parsing hex node_id '{}': {}", args.node_id, e);
            std::process::exit(1);
        }
    };
}