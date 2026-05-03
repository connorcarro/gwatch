use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::Instant;

const CHARS_PER_LINE: usize = 20;
const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

struct FastRng {
    state: u64,
}

impl FastRng {
    fn new() -> Self {
        Self { state: 0x123456789abcdef0 }
    }

    #[inline(always)]
    fn next(&mut self) -> u64 {
        // SplitMix64 – extremely fast, high-quality PRNG (much faster than Python's random)
        self.state = self.state.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <millions> [output_file]", args[0]);
        eprintln!("  millions: number of millions of lines (e.g. 2 for 2,000,000 lines)");
        std::process::exit(1);
    }

    let millions: u64 = match args[1].parse() {
        Ok(n) => n,
        Err(_) => {
            eprintln!("Error: '{}' is not a valid number", args[1]);
            std::process::exit(1);
        }
    };

    let output = if args.len() > 2 { &args[2] } else { "output.txt" };
    let total_lines = millions * 1_000_000;

    let start = Instant::now();

    let file = File::create(output).expect("failed to create output file");
    // 64 MiB buffer (much larger than Python's 8 MiB) for maximum write throughput
    let mut writer = BufWriter::with_capacity(64 * 1024 * 1024, file);

    let mut rng = FastRng::new();
    let charset_len = CHARSET.len() as u64;

    // Reusable stack-allocated line buffer – zero allocations per line
    let mut line = [0u8; CHARS_PER_LINE + 1];
    line[CHARS_PER_LINE] = b'\n';

    for _ in 0..total_lines {
        for i in 0..CHARS_PER_LINE {
            let idx = (rng.next() % charset_len) as usize;
            line[i] = CHARSET[idx];
        }
        writer.write_all(&line).expect("write failed");
    }

    writer.flush().expect("flush failed");

    let elapsed = start.elapsed();
    let seconds = elapsed.as_secs_f64();

    println!("Finished writing {} in {:.2} seconds", output, seconds);

    // Bonus: show real throughput (not in original Python)
    let mb_written = (total_lines * (CHARS_PER_LINE + 1) as u64) as f64 / (1024.0 * 1024.0);
    println!(
        "Throughput: {:.1} MB/s  ({:.2} million lines/sec)",
        mb_written / seconds,
        (total_lines as f64 / 1_000_000.0) / seconds
    );
}