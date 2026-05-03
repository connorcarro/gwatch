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
        eprintln!("  millions: number of millions of lines (e.g. 2, 1.5, 0.25)");
        std::process::exit(1);
    }

    let input = &args[1];
    let millions: f64 = match input.parse::<f64>() {
        Ok(n) if n > 0.0 => n,
        Ok(_) => {
            eprintln!("Error: millions must be greater than 0");
            std::process::exit(1);
        }
        Err(_) => {
            // Support both . and , as decimal separator
            let fixed = input.replace(',', ".");
            match fixed.parse::<f64>() {
                Ok(n) if n > 0.0 => n,
                _ => {
                    eprintln!("Error: '{}' is not a valid number", input);
                    std::process::exit(1);
                }
            }
        }
    };

    let output = if args.len() > 2 { &args[2] } else { "output.txt" };
    let total_lines = (millions * 1_000_000.0).round() as u64;

    if total_lines == 0 {
        eprintln!("Error: resulting line count would be zero");
        std::process::exit(1);
    }

    let start = Instant::now();
    let file = File::create(output).expect("failed to create output file");
    let mut writer = BufWriter::with_capacity(64 * 1024 * 1024, file);

    let mut rng = FastRng::new();
    let charset_len = CHARSET.len() as u64;

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

    println!("Finished writing {} lines to {} in {:.2}s", total_lines, output, seconds);
    let mb_written = (total_lines * (CHARS_PER_LINE + 1) as u64) as f64 / (1024.0 * 1024.0);
    println!(
        "Throughput: {:.1} MB/s  ({:.2} million lines/sec)",
        mb_written / seconds,
        (total_lines as f64 / 1_000_000.0) / seconds
    );
}