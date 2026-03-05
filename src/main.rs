use anyhow::anyhow;
use clap::Parser;
use embedded_can::{blocking::Can, ExtendedId, Frame as EmbeddedFrame};
use socketcan::{CanFdFrame, CanFdSocket, Result, Socket};
use std::path::{Path, PathBuf};
use std::{env, fs};
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
    /*
    for i in 0..10{
        println!("Byte {} = {}",i,hex::encode(&[binvec[i]]));
    }
    */

    let nodeid: u32 = match u32::from_str_radix(&args.node_id, 16) {
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
    /*
    println!("Node ID: {:05b}", nodeid);
    let test_id = dfr_can_id(5,nodeid,5,1).expect("Error creating CAN ID");

    println!("ID as decimal {}", test_id.as_raw());
    println!("ID as binary: {:029b}",test_id.as_raw());
    println!("ID as hex {:08X}", test_id.as_raw());
    */
    match write_binary(&binvec, &sock, 0x06, 0x01) {
        Ok(_) => {
            println!("Successfully wrote binary :D");
        }
        Err(e) => {
            anyhow::bail!(e);
        }
    }

    Ok(())
}

fn dfr_can_id(
    priority: u32,
    target: u32,
    command: u32,
    source: u32,
) -> anyhow::Result<embedded_can::ExtendedId> {
    // makes an extended id with [3 bits priority][5 bit target id][16 bit command][5 bit source id]
    if priority > 7 {
        return Err(anyhow::format_err!("Priority {} is out of range", priority));
    } else if target > 31 {
        return Err(anyhow::format_err!("Target ID {} is out of range", target));
    } else if command > 65535 {
        return Err(anyhow::format_err!("Command {} is out of range", command));
    } else if source > 31 {
        return Err(anyhow::format_err!("Source {} is out of range", source));
    }
    let id_u32 = (priority << 26) | (target << 21) | (command << 5) | source;
    Ok(embedded_can::ExtendedId::new(id_u32).unwrap())
}
fn write_binary(
    binv: &Vec<u8>,
    tx: &CanFdSocket,
    targetid: u32,
    sourceid: u32,
) -> anyhow::Result<()> {
    for (i, chunk) in binv.chunks(64).enumerate() {
        let id = dfr_can_id(1, targetid, 0xAAAA, sourceid)?;
        if let Some(frame) = socketcan::CanFdFrame::new(id, chunk) {
            tx.write_frame(&frame)?;
            //println!("Sent chunk {} with {} bytes", i, chunk.len());
        }
    }

    Ok(())
}
