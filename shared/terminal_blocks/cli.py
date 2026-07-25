#!/usr/bin/env python3

import sys

sys.dont_write_bytecode = True

from blocks import select_blocks


def main() -> int:
    requested = sys.argv[1] if len(sys.argv) > 1 else None
    try:
        selection = select_blocks(sys.stdin.read(), requested)
    except ValueError as error:
        print(error, file=sys.stderr)
        return 2
    if selection.count == 0:
        return 1
    sys.stdout.write(selection.text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
