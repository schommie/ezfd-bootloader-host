use anyhow::anyhow;
use clap::Parser;
use embedded_can::{Id, Frame as EmbeddedFrame};
use indicatif::{ProgressBar, ProgressStyle};
use socketcan::{CanFdFrame, CanFdSocket, Socket};
use std::path::{Path, PathBuf};
use std::time::Duration;
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

fn send_frame(sock: &CanFdSocket, target: u16, command: BootloaderCommand, source: u16, data: &[u8]) -> anyhow::Result<()> {
    let id = DfrCanId::new(1, target, command.into(), source)
        .map_err(|e| anyhow::format_err!(e))?;
    let ext_id = embedded_can::ExtendedId::new(id.to_raw_id()).unwrap();
    if let Some(frame) = CanFdFrame::new(ext_id, data) {
        sock.write_frame(&frame)?;
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let sock = match CanFdSocket::open(&args.interface) {
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

    println!("Pinging device 0x{:02X}...", nodeid);
    sock.set_read_timeout(Duration::from_millis(500))?;
    send_frame(&sock, nodeid, BootloaderCommand::Ping, sourceid, &[])?;

    let mut device_in_bootloader = false;
    let ping_deadline = std::time::Instant::now() + Duration::from_millis(500);
    while std::time::Instant::now() < ping_deadline {
        match sock.read_frame() {
            Ok(rx_frame) => {
                if let Id::Extended(ext_id) = rx_frame.id() {
                    let msg_id = parse_can_id(ext_id.as_raw());
                    if msg_id.target == sourceid
                        && msg_id.source == nodeid
                        && msg_id.command == BootloaderCommand::Ping.into()
                    {
                        let data = rx_frame.data();
                        let status = data.first().copied().unwrap_or(0xFF);
                        if status == 0 {
                            println!("Device is in bootloader mode.");
                            device_in_bootloader = true;
                        } else if status == 1 {
                            println!("Device is in application mode.");
                        } else {
                            println!("Unexpected ping status byte: 0x{:02X}", status);
                        }
                        break;
                    }
                }
            }
            Err(_) => break,
        }
    }

    if !device_in_bootloader {
        println!("Sending Reboot command...");
        send_frame(&sock, nodeid, BootloaderCommand::Reboot, sourceid, &[])?;

        println!("Waiting for device FirmwareUpdateQuery (power-cycle the device if needed)...");
        sock.set_read_timeout(Duration::from_secs(5))?;
        loop {
            match sock.read_frame() {
                Ok(rx_frame) => {
                    if let Id::Extended(ext_id) = rx_frame.id() {
                        let msg_id = parse_can_id(ext_id.as_raw());
                        if msg_id.target == sourceid
                            && msg_id.source == nodeid
                            && msg_id.command == BootloaderCommand::FirmwareUpdateQuery.into()
                        {
                            println!("Received FirmwareUpdateQuery, responding with update=true");
                            send_frame(&sock, nodeid, BootloaderCommand::FirmwareUpdateResponse, sourceid, &[1u8])?;
                            break;
                        }
                    }
                }
                Err(_) => {
                    print!(".");
                    use std::io::Write;
                    std::io::stdout().flush().ok();
                }
            }
        }
    }

    sock.set_read_timeout(Duration::from_secs(30))?;

    println!("Sending Erase command...");
    send_frame(&sock, nodeid, BootloaderCommand::Erase, sourceid, &[])?;

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

    match write_binary(&binvec, &sock, nodeid, sourceid) {
        Ok(_) => {
            println!("Successfully wrote binary :D");
            println!("Sending jump command...");
            send_frame(&sock, nodeid, BootloaderCommand::Jump, sourceid, &[])?;
        }
        Err(e) => {
            anyhow::bail!(e);
        }
    }

    Ok(())
}

fn write_binary(
    binv: &Vec<u8>,
    sock: &CanFdSocket,
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

        send_frame(sock, targetid, BootloaderCommand::AddressAndSize, sourceid, &payload)?;
        send_frame(sock, targetid, BootloaderCommand::Write, sourceid, chunk)?;

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
