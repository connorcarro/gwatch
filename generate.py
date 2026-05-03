from pathlib import Path
import argparse
import random
import string
import time

CHARS_PER_LINE = 20


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "millions",
        type=int,
        help="Number of millions of lines to generate (e.g. 2 = 2,000,000 lines)",
    )
    parser.add_argument(
        "output",
        nargs="?",
        default="output.txt",
        help="Output text file path",
    )
    args = parser.parse_args()

    start_time = time.perf_counter()

    total_lines = args.millions * 1_000_000
    output_path = Path(args.output)

    charset = string.ascii_letters + string.digits

    with output_path.open("wb", buffering=1024 * 1024 * 8) as f:
        buffer = []
        buffer_size = 0

        for _ in range(total_lines):
            line = ''.join(random.choices(charset, k=CHARS_PER_LINE)) + '\n'
            line_bytes = line.encode()
            buffer.append(line_bytes)
            buffer_size += len(line_bytes)

            if buffer_size > 10 * 1024 * 1024:
                f.write(b"".join(buffer))
                buffer.clear()
                buffer_size = 0

        if buffer:
            f.write(b"".join(buffer))

    elapsed = time.perf_counter() - start_time
    print(f"Finished writing {output_path} in {elapsed:.2f} seconds")


if __name__ == "__main__":
    main()
