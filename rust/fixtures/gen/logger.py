"""Golden cases for beam_base::logger: what Python's RotatingFileHandler (as bnh_logger
configures it, but with a small maxBytes) leaves on disk for a fixed message sequence."""

from __future__ import annotations

import base64
import logging
import logging.handlers
import os
import shutil
import tempfile

from _common import write_fixture

MESSAGES = (
    ["short"] * 3
    + ["é" * 40, "a\nmulti-line\nmessage", "x" * 95, "y" * 5, "ünï" * 10]
    + [f"line {i}" for i in range(30)]
    + ["z" * 300, "after a long one", "" , "end"]
)


def run(max_bytes, backups):
    d = tempfile.mkdtemp(prefix="beam_log_fx_")
    path = os.path.join(d, "bnvdahook.log")
    logger = logging.getLogger(f"fixture.{max_bytes}.{backups}")
    logger.propagate = False
    logger.setLevel(logging.INFO)
    fh = logging.handlers.RotatingFileHandler(
        path, maxBytes=max_bytes, backupCount=backups, encoding="utf-8"
    )
    fh.setFormatter(logging.Formatter("%(message)s"))
    logger.addHandler(fh)
    for m in MESSAGES:
        logger.info(m)
    fh.close()
    logger.removeHandler(fh)
    files = {}
    for name in sorted(os.listdir(d)):
        with open(os.path.join(d, name), "rb") as f:
            files[name] = base64.b64encode(f.read()).decode("ascii")
    shutil.rmtree(d)
    return files


def main():
    write_fixture("logger", {
        "messages": MESSAGES,
        "runs": [{"max_bytes": mb, "backups": bc, "files": run(mb, bc)}
                 for mb, bc in [(200, 3), (100, 1), (1000, 0), (0, 3), (50, 2)]],
        "format": logging.Formatter(
            "%(asctime)s [%(levelname)s] %(message)s", datefmt="%Y-%m-%d %H:%M:%S"
        )._fmt,
        "levels": {str(lv): logging.getLevelName(lv) for lv in (10, 20, 30, 40)},
    })


if __name__ == "__main__":
    main()
