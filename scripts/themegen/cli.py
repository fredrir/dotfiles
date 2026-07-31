import argparse

from .model import Theme
from .registry import EMITTERS
from .render import Output


def build_parser():
    parser = argparse.ArgumentParser(
        prog="generate-theme.py",
        description="Stamp theme/palette.toml into every config that carries colors.",
    )
    parser.add_argument(
        "--list-outputs",
        action="store_true",
        help="print the files this generator owns, one per line, and exit",
    )
    parser.add_argument(
        "--stageable",
        action="store_true",
        help="with --list-outputs, list only files that are safe to stage automatically",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="report what would change without writing, and exit non-zero if anything would",
    )
    return parser


def main(argv=None):
    args = build_parser().parse_args(argv)

    if args.list_outputs:
        for emitter in EMITTERS:
            if args.stageable and not emitter.stageable:
                continue
            for target in emitter.outputs():
                print(target)
        return 0

    theme = Theme.load()
    out = Output(check=args.check)
    for emitter in EMITTERS:
        emitter.run(theme, out)

    if not out.changed:
        print("theme: already up to date")
        return 0

    print("theme: would regenerate" if args.check else "theme: regenerated")
    for target in out.changed:
        print(f"  {target}")
    return 1 if args.check else 0
