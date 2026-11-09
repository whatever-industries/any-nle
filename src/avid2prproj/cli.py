"""avid2prproj: write a native Premiere project from a neutral edit model (JSON).

    python -m avid2prproj MODEL.json OUTPUT.prproj [--bin-mode separate|merged|grouped]
"""
from __future__ import annotations

import argparse
from dataclasses import replace
import json
import sys
from pathlib import Path

from .model import BIN_MODES, ModelError, load_model
from .opatom import UnverifiedShape
from .writer import write_project


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="avid2prproj", description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("model", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--bin-mode", choices=BIN_MODES, help="override the model's bin mode")
    parser.add_argument("--match-avid-default-pan", action="store_true",
                        help="pan mono edits with no explicit Avid pan the way Avid plays them (odd tracks left, even right)")
    args = parser.parse_args(argv)
    try:
        model = load_model(args.model)
        options = model.options
        if args.bin_mode:
            options = replace(options, bin_mode=args.bin_mode)
        if args.match_avid_default_pan:
            options = replace(options, match_avid_default_pan=True)
        report = write_project(replace(model, options=options), args.output)
    except (ModelError, UnverifiedShape, FileExistsError, FileNotFoundError) as error:
        print(f"avid2prproj: {error}", file=sys.stderr)
        return 2
    print(json.dumps({"output": str(args.output), **report.as_dict()}, indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
