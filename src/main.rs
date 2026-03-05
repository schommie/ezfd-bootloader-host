use anyhow::anyhow;
use clap::Parser;
use embedded_can::{blocking::Can, ExtendedId, Frame as EmbeddedFrame};
use socketcan::{CanFdFrame, CanFdSocket, Result, Socket};
use std::path::{Path, PathBuf};
use std::{env, fs};

mod protocol;
use protocol::{BootloaderCommand, DfrCanId};

#[derive(Parser, Debug)]
struct Args {
    #[arg(short, long)]
    binary: PathBuf,

    #[arg(short, long, default_value = "can0")]
    interface: String,

    #[arg(short, long)]
    node_id: String,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let mut sock = match CanFdSocket::open(&args.interface) {
        Ok(sock) => {
            println!("Successfully opened interface on {}", args.interface);
            sock
        }
        Err(e) => {
            return Err(anyhow!(e));
        }
    };

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
    let binvec: Vec<u8> = match fs::read(&resolved_path) {
        Ok(bytes) => {
            println!("Success! Read {} bytes.", bytes.len());
            bytes
        }
        Err(e) => {
            return Err(anyhow::format_err!(
                "Error reading binary file {:?}: {}",
                resolved_path,
                e
            ));
        }
    };

    let nodeid: u16 = match u16::from_str_radix(&args.node_id, 16) {
        Ok(id) if id <= 0x1F => {
            println!("Attempting to connect to Node ID: 0x{:02X} ({})", id, id);
            id
        }
        Ok(_) => {
            return Err(anyhow::format_err!("ID {} Out of range", args.node_id));
        }
        Err(e) => {
            return Err(anyhow::format_err!(
                "Error parsing hex node_id '{}': {}",
                args.node_id,
                e
            ));
        }
    };

    match write_binary(&binvec, &sock, nodeid, 0x01) {
        Ok(_) => {
            println!("Successfully wrote binary :D");
        }
        Err(e) => {
            anyhow::bail!(e);
        }
    }

    Ok(())
}

fn write_binary(
    binv: &Vec<u8>,
    tx: &CanFdSocket,
    targetid: u16,
    sourceid: u16,
) -> anyhow::Result<()> {
    for (i, chunk) in binv.chunks(64).enumerate() {
        let dfr_id = DfrCanId::new(1, targetid, BootloaderCommand::Write.into(), sourceid)
            .map_err(|e| anyhow::format_err!(e))?;
        let extended_id = embedded_can::ExtendedId::new(dfr_id.to_raw_id()).unwrap();
        if let Some(frame) = socketcan::CanFdFrame::new(extended_id, chunk) {
            tx.write_frame(&frame)?;
        }
    }

    Ok(())
}
