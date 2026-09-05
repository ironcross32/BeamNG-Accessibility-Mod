import socket, struct, time, sys

ADDR = "127.0.0.1"
PORT = 5246
SOCK_TIMEOUT = 0.5

# Bits from your Lua
OG_TURBO = 8192
OG_KM = 16384
OG_BAR = 32768

DL_SHIFT = 1 << 0
DL_FULLBEAM = 1 << 1
DL_HANDBRAKE = 1 << 2
DL_PITSPEED = 1 << 3
DL_TC = 1 << 4
DL_SIGNAL_L = 1 << 5
DL_SIGNAL_R = 1 << 6
DL_CHECK = 1 << 7
DL_OILWARN = 1 << 8
DL_BATTERY = 1 << 9
DL_ABS = 1 << 10
DL_LOWBEAM = 1 << 11

LIGHT_NAMES = [
    ("SHIFT", DL_SHIFT),
    ("FULLBEAM", DL_FULLBEAM),
    ("HANDBRAKE", DL_HANDBRAKE),
    ("PITSPEED", DL_PITSPEED),
    ("TC", DL_TC),
    ("SIGNAL_L", DL_SIGNAL_L),
    ("SIGNAL_R", DL_SIGNAL_R),
    ("CHECK", DL_CHECK),
    ("OILWARN", DL_OILWARN),
    ("BATTERY", DL_BATTERY),
    ("ABS", DL_ABS),
    ("LOWBEAM", DL_LOWBEAM),
]

# Two candidate layouts:
#  - Without padding after plid (some protocols do this)
#  - With 1 pad byte 'x' after plid to align floats (common C-struct alignment)
FMTS = [
    ("<H4sB9fII3f", struct.calcsize("<H4sB9fII3f")),
    ("<H4sBx9fII3f", struct.calcsize("<H4sBx9fII3f")),
]


def _plausible(rec):
    """Quick sanity checks to pick the right struct."""
    # Index mapping:
    #  0 flags(H), 1 gear(4s), 2 plid(B),
    #  3 speed, 4 rpm, 5 rpmMax, 6 turbo, 7 turboMax, 8 engTemp,
    #  9 fuel, 10 oilPressure, 11 oilTemp,
    #  12 dash, 13 show, 14 throttle, 15 brake, 16 clutch
    try:
        speed = rec[3]
        rpm = rec[4]
        fuel = rec[9]
        if not (-200.0 <= speed <= 200.0):
            return False
        if not (0.0 <= rpm <= 20000.0):
            return False
        if not (-0.1 <= fuel <= 1.1):
            return False
        return True
    except Exception:
        return False


def _decode(data):
    for fmt, size in FMTS:
        if len(data) < size:
            continue
        try:
            rec = struct.unpack(fmt, data[:size])
        except struct.error:
            continue
        if _plausible(rec):
            return rec, fmt
    return None, None


def _bits_to_names(bits):
    avail = []
    for name, mask in LIGHT_NAMES:
        if (bits & mask) != 0:
            avail.append(name)
    return ",".join(avail) if avail else "-"


def main():
    print(f"[INFO] Listening on {ADDR}:{PORT} (Ctrl+C to quit)")
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.bind((ADDR, PORT))
    sock.settimeout(SOCK_TIMEOUT)

    chosen_fmt = None

    try:
        while True:
            try:
                pkt, _ = sock.recvfrom(2048)
            except TimeoutError:
                continue
            except KeyboardInterrupt:
                print("\n[INFO] Stopping.")
                break

            rec, fmt = _decode(pkt)
            if rec is None:
                # Couldn’t parse sensibly; show raw size and continue
                print(f"[WARN] Unrecognized packet of {len(pkt)} bytes")
                continue

            if chosen_fmt is None:
                chosen_fmt = fmt
                print(f"[INFO] Using struct format: {fmt}")

            flags = rec[0]
            gear = rec[1].split(b"\x00", 1)[0].decode("ascii", "ignore") or "?"
            # rec[2] = plid (unused)

            speed, rpm, rpmMax, turbo, turboMax, engTemp, fuel, oilPressure, oilTemp = (
                rec[3:12]
            )
            dashLights, showLights = rec[12], rec[13]
            throttle, brake, clutch = rec[14], rec[15], rec[16]

            flags_str = f"km={bool(flags & OG_KM)}, bar={bool(flags & OG_BAR)}, turbo_ui={bool(flags & OG_TURBO)}"
            avail_str = _bits_to_names(dashLights)
            on_str = _bits_to_names(showLights)

            print(
                f"gear={gear:<3}  "
                f"speed={speed:6.2f} m/s  "
                f"rpm={rpm:.0f}/{rpmMax:.0f}  "
                f"boost={turbo:.2f}/{turboMax:.2f} bar  "
                f"water={engTemp:.1f}°C  oil={oilTemp:.1f}°C  "
                f"fuel={fuel*100:.1f}%  "
                f"thr/brk/cl={throttle:.2f}/{brake:.2f}/{clutch:.2f}  "
                f"flags({flags_str})  "
                f"lights: avail={avail_str}; on={on_str}"
            )

    except KeyboardInterrupt:
        print("\n[INFO] Stopping.")
    finally:
        try:
            sock.close()
        except Exception:
            pass


if __name__ == "__main__":
    main()
