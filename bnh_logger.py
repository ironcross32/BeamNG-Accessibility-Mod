# bnh_logger.py
# Centralized logger for the BeamNG NVDA Hook project.

import logging
import logging.handlers
import os
import sys

# Determine the base directory to store the log file
FROZEN = getattr(sys, "frozen", False)
BASE_DIR = (
    os.path.dirname(os.path.abspath(sys.executable))
    if FROZEN
    else os.path.dirname(os.path.abspath(__file__))
)
LOG_FILENAME = os.path.join(BASE_DIR, "bnvdahook.log")

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
