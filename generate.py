from pathlib import Path
import argparse
import random
import string

LINES = 50_000_000
CHARS_PER_LINE = 20
CHUNK_LINES = 1_000_000


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "output",
        nargs="?",
        default="output.txt",
        help="Output text file path",
    )
    args = parser.parse_args()

    output_path = Path(args.output)
    
    # Pre-generate character set
    charset = string.ascii_letters + string.digits

    with output_path.open("wb", buffering=1024 * 1024 * 8) as f:
        buffer = []
        buffer_size = 0
        
        for _ in range(LINES):
            # Generate line without function call overhead
            line = ''.join(random.choices(charset, k=CHARS_PER_LINE)) + '\n'
            line_bytes = line.encode()
            buffer.append(line_bytes)
            buffer_size += len(line_bytes)
            
            # Flush buffer periodically
            if buffer_size > 10 * 1024 * 1024:  # 10MB chunks
                f.write(b''.join(buffer))
                buffer.clear()
                buffer_size = 0
        
        # Write remaining
        if buffer:
            f.write(b''.join(buffer))


if __name__ == "__main__":
    main()
