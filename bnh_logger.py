# bnh_logger.py
# Centralized logger for the BeamNG NVDA Hook project.

import logging
import logging.handlers
import os
import sys

# Store the log file in %LOCALAPPDATA%\beamtel alongside other app data
def _get_log_dir():
    base_path = os.getenv("LOCALAPPDATA")
    if base_path is None:
        base_path = os.path.expanduser("~")
    log_dir = os.path.join(base_path, "beamtel")
    os.makedirs(log_dir, exist_ok=True)
    return log_dir

LOG_DIR = _get_log_dir()
LOG_FILENAME = os.path.join(LOG_DIR, "bnvdahook.log")

_LOGGER = None


def get_logger():
    """
    Sets up and returns a shared, singleton logger instance.
    """
    global _LOGGER
    if _LOGGER:
        return _LOGGER

    # Use a unique name for the logger to get the same instance everywhere
    logger = logging.getLogger("bnvdahook")

    # Check if handlers are already configured to prevent duplicates
    if not logger.handlers:
        logger.setLevel(logging.INFO)

        # File handler that rotates logs
        fh = logging.handlers.RotatingFileHandler(
            LOG_FILENAME, maxBytes=1_024_000, backupCount=3, encoding="utf-8"
        )

        # Console handler for real-time output
        sh = logging.StreamHandler()

        # Formatter to define the log message structure
        fmt = logging.Formatter(
            "%(asctime)s [%(levelname)s] %(message)s", datefmt="%Y-%m-%d %H:%M:%S"
        )
        fh.setFormatter(fmt)
        sh.setFormatter(fmt)

        logger.addHandler(fh)
        logger.addHandler(sh)

        logger.info("=" * 20 + " Logger Initialized " + "=" * 20)

    _LOGGER = logger
    return logger
