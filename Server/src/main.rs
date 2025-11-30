use std::net::UdpSocket;
use std::collections::HashMap;
use image::io::Reader as ImageReader;
use image::ImageFormat;
use std::time::{Duration, SystemTime};
use std::thread::sleep;

const BUFFER_SIZE: usize = 1*1024*12; // 1byte * 1024 kB * 12 mB

struct FrameBuffer {
    total_chunks: u32,
    received_chunks: HashMap<usize, Vec<u8>>,
    chunks_count: usize,
}

async fn display_image(data: &Vec<u8>, frame_: u64) {
    // Try to decode image
    match ImageReader::new(std::io::Cursor::new(data)).with_guessed_format() {
        Ok(reader) => {
            match reader.decode() {
                Ok(img) => {
                    // Save image with unique filename
                    let filename = format!("./frames/frame_{}.png", frame_);
                    // Ensure the directory exists
                    std::fs::create_dir_all("frames").unwrap();
                    img.save(&filename).unwrap();
                    println!("Image saved as '{}'", filename);
                }
                Err(e) => {
                    eprintln!("Error decoding image: {}", e);
                }
            }
        }
        Err(e) => {
            eprintln!("Could not determine image format: {}", e);
        }
    }
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let socket = UdpSocket::bind("127.0.0.1:11856")?;
    let mut buffer = [0u8; BUFFER_SIZE];
    let mut frames: HashMap<u32, FrameBuffer> = HashMap::new();
    let mut frame_: u64 = 0;
    let now = SystemTime::now();
    loop {
        let (len, src) = socket.recv_from(&mut buffer)?;
        if len < 8 {
            continue; // ignore invalid
        }

        // Parse header
        let total_chunks = u32::from_be_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]);
        let chunk_idx = u32::from_be_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]);
        let data_chunk = &buffer[8..len];

        // Get existing frame buffer or create new
        let frame_entry = frames.entry(total_chunks).or_insert_with(|| FrameBuffer {
            total_chunks,
            received_chunks: HashMap::new(),
            chunks_count: total_chunks as usize,
        });

        // Insert chunk
        frame_entry.received_chunks.insert(chunk_idx as usize, data_chunk.to_vec());

        // Check if frame is complete
        if frame_entry.received_chunks.len() == frame_entry.chunks_count {
            let mut full_data = Vec::new();
            for idx in 0..frame_entry.chunks_count {
                if let Some(chunk) = frame_entry.received_chunks.get(&idx) {
                    full_data.extend_from_slice(chunk);
                } else {
                    eprintln!("Missing chunk {} in frame", idx);
                }
            }
            frame_ += 1;
            println!("Frame assembled, size: {}, frame #{}, FPS: {}", full_data.len(), frame_, ((frame_*1000) as u128)/now.elapsed().unwrap().as_millis());
            display_image(&full_data, frame_).await;
            frames.remove(&total_chunks);
        }
    }
}