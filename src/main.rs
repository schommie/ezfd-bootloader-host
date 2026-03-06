use anyhow::anyhow;
use clap::Parser;
use embedded_can::{Id, Frame as EmbeddedFrame};
use indicatif::{ProgressBar, ProgressStyle};
use socketcan::{CanFdFrame, CanFdSocket, Socket};
use std::path::{Path, PathBuf};
use std::fs;

mod protocol;
use protocol::*;

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

    let sourceid = CanDevices::RaspberryPi as u16;

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
    println!("Sending Erase command...");

    let id_erase = DfrCanId::new(1, nodeid, BootloaderCommand::Erase.into(), sourceid)
        .map_err(|e| anyhow::format_err!(e))?;

    let ext_id_erase = embedded_can::ExtendedId::new(id_erase.to_raw_id()).unwrap();

    if let Some(frame_erase) = CanFdFrame::new(ext_id_erase, &[]) {
        sock.write_frame(&frame_erase)?;
    }

    println!("Waiting for device to erase flash...");
    loop {
        let rx_frame = sock.read_frame()?;

        if let embedded_can::Id::Extended(ext_id) = rx_frame.id() {
            let msg_id = parse_can_id(ext_id.as_raw());

            if msg_id.target == sourceid
                && msg_id.source == nodeid
                && msg_id.command == BootloaderCommand::EraseOk.into()
            {
                println!("Received EraseOk! Flash is ready.");
                break;
            }
        }
    }

    match write_binary(&binvec, &mut sock, nodeid, CanDevices::RaspberryPi as u16) {
        Ok(_) => {
            println!("Successfully wrote binary :D");
            println!("Sending jump command...");
            let id_jump = DfrCanId::new(1, nodeid, BootloaderCommand::Jump.into(), sourceid)
                .map_err(|e| anyhow::format_err!(e))?;
            let ext_id_jump = embedded_can::ExtendedId::new(id_jump.to_raw_id()).unwrap();

            if let Some(frame_jump) = CanFdFrame::new(ext_id_jump, &[]) {
                sock.write_frame(&frame_jump)?;
            }

        }
        Err(e) => {
            anyhow::bail!(e);
        }
    }

    Ok(())
}

fn write_binary(
    binv: &Vec<u8>,
    sock: &mut CanFdSocket,
    targetid: u16,
    sourceid: u16,
) -> anyhow::Result<()> {


    let base_address: u32 = 0x0800_8000;

    let total_chunks = binv.chunks(64).count();
    let pb = ProgressBar::new(total_chunks as u64);
    pb.set_style(
        ProgressStyle::with_template("[{elapsed_precise}] [{bar:40}] {pos}/{len} chunks ({eta})")
            .unwrap()
            .progress_chars("=>-"),
    );

    for (i, chunk) in binv.chunks(64).enumerate() {
        let chunk_address = base_address + (i * 64) as u32;
        let chunk_size = chunk.len() as u8;

        let addr_bytes = chunk_address.to_be_bytes();
        let payload = [
            addr_bytes[0],
            addr_bytes[1],
            addr_bytes[2],
            addr_bytes[3],
            chunk_size,
        ];

        let id_addr = DfrCanId::new(1, targetid, BootloaderCommand::AddressAndSize.into(), sourceid)
            .map_err(|e| anyhow::format_err!(e))?;
        let ext_id_addr = embedded_can::ExtendedId::new(id_addr.to_raw_id()).unwrap();

        if let Some(frame_addr) = CanFdFrame::new(ext_id_addr, &payload) {
            sock.write_frame(&frame_addr)?;
        }

        let id_write = DfrCanId::new(1, targetid, BootloaderCommand::Write.into(), sourceid)
            .map_err(|e| anyhow::format_err!(e))?;
        let ext_id_write = embedded_can::ExtendedId::new(id_write.to_raw_id()).unwrap();

        if let Some(frame_write) = CanFdFrame::new(ext_id_write, chunk) {
            sock.write_frame(&frame_write)?;
            //println!("Sent chunk {} ({} bytes) at address 0x{:08X}", i, chunk_size, chunk_address);
        }
        loop {
            let rx_frame = sock.read_frame()?;

            if let Id::Extended(ext_id) = rx_frame.id() {
                let msg_id = parse_can_id(ext_id.as_raw());
                if msg_id.target == sourceid
                    && msg_id.source == targetid
                    && msg_id.command == BootloaderCommand::WriteOk.into()
                {
                    let data = rx_frame.data();

                    if data.len() >= 5 {
                        let ack_addr = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
                        let ack_size = data[4];

                        if ack_addr == chunk_address && ack_size == chunk_size {
                            pb.inc(1);
                            break;
                        } else {
                            return Err(anyhow::format_err!(
                                "WriteOk mismatch! Expected Addr: 0x{:08X}, Size: {}. Got Addr: 0x{:08X}, Size: {}",
                                chunk_address, chunk_size, ack_addr, ack_size
                            ));
                        }
                    }
                }
            }
        }
    }

    pb.finish_with_message("done");
    Ok(())
}
