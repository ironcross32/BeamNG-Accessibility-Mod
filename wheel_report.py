"""Wheel rows for the status screen: every wheel's position label, pressure and brake temperature.

Pure like gear_report.py and wheel_slip.py. wheelReport.lua sends each vehicle's raw wheel
list -- longitudinal and lateral position in the vehicle's own frame, tire width, pressure,
brake temperature -- and this module decides what each wheel is called, so
diagnostic/wheel_report_sim.py can assert the labels with no game.

Labels come from geometry, never from jbeam names, which differ per vehicle: the 0.40
Percheron's wheels are FL FR R2LL R2RR TLL TLR. A two-axle vehicle reads exactly as the
fixed-slot rows always did ("front left" ... "rear right"); more axles number the middle
ones ("front", "axle 2", "rear"); a coupled vehicle is prefixed "trailer" ("trailer 2" when
there is more than one).
"""

# Wheels whose centres are closer than this along the vehicle are one axle. Duals share a
# centre exactly; the closest real tandem axles are well over a metre apart.
AXLE_GAP_M = 0.5
# Inside this of the centreline a wheel has no side (a motorcycle, a trike's nose wheel).
CENTRE_LAT_M = 0.2
# With the jbeam's simplifiedDuallies switch on, a dual pair is ONE physics wheel about twice
# as wide as a single (0.59 m against 0.265 on the Percheron), and only the width says so.
DUAL_WIDTH_RATIO = 1.6


def _float_or_none(text):
    text = text.strip()
    if not text:
        return None
    return float(text)


def parse_packet(text):
    """Parse a WHEELS packet into vehicles, or None if it is not one.

    Each vehicle is {"role", "id", "name", "dual", "wheels"}; each wheel is
    {"lon", "lat", "width", "psi", "brake_c", "broken"} with psi / brake_c None when the
    game has no such reading. A malformed wheel is skipped rather than failing the vehicle.
    """
    lines = text.strip().split("\n")
    if not lines or lines[0].strip() != "WHEELS":
        return None
    vehicles = []
    i = 1
    while i < len(lines):
        head = lines[i].strip()
        i += 1
        if not head.startswith("V,"):
            continue
        parts = head.split(",", 3)
        if len(parts) < 3:
            continue
        try:
            role, vid = int(parts[1]), int(parts[2])
        except ValueError:
            continue
        name = parts[3].strip() if len(parts) >= 4 else ""
        body = lines[i].strip() if i < len(lines) else ""
        if body.startswith("V,"):
            body = ""
        else:
            i += 1
        dual_flag, _, wheel_text = body.partition("|")
        wheels = []
        for item in wheel_text.split(";"):
            fields = item.split(",")
            if len(fields) < 6:
                continue
            try:
                wheels.append(
                    {
                        "lon": float(fields[0]),
                        "lat": float(fields[1]),
                        "width": float(fields[2]),
                        "psi": _float_or_none(fields[3]),
                        "brake_c": _float_or_none(fields[4]),
                        "broken": fields[5].strip() == "1",
                    }
                )
            except ValueError:
                continue
        vehicles.append(
            {
                "role": role,
                "id": vid,
                "name": name,
                "dual": dual_flag.strip() == "1",
                "wheels": wheels,
            }
        )
    return vehicles


def _axle_names(count):
    if count <= 1:
        return [""]
    if count == 2:
        return ["front", "rear"]
    return ["front"] + [f"axle {n}" for n in range(2, count)] + ["rear"]


def _stack_names(count):
    """Names for wheels sharing one axle end, innermost first."""
    if count == 1:
        return [""]
    if count == 2:
        return ["inner", "outer"]
    if count == 3:
        return ["inner", "middle", "outer"]
    return [str(n) for n in range(1, count + 1)]


def _vehicle_prefix(role, trailer_count):
    if role == 0:
        return ""
    return "trailer" if trailer_count <= 1 else f"trailer {role}"


def vehicle_rows(vehicle, trailer_count=1):
    """One row per wheel, front to back and left to right: {"label", "psi", "brake_c", "broken"}."""
    wheels = sorted(vehicle["wheels"], key=lambda w: -w["lon"])
    if not wheels:
        return []
    # Positions are relative to the vehicle's origin, which need not be on its centreline: the
    # ATV's wheels arrive at lateral -0.4 and +1.7. Centre on the middle of the wheel spread,
    # which also keeps a trike's single wheel in the middle.
    lats = [w["lat"] for w in wheels]
    mid = (min(lats) + max(lats)) / 2.0
    wheels = [dict(w, lat=w["lat"] - mid) for w in wheels]
    axles = [[wheels[0]]]
    for w in wheels[1:]:
        if axles[-1][-1]["lon"] - w["lon"] > AXLE_GAP_M:
            axles.append([w])
        else:
            axles[-1].append(w)

    widths = [w["width"] for w in wheels if w["width"] > 0]
    narrowest = min(widths) if widths else 0.0
    prefix = _vehicle_prefix(vehicle["role"], trailer_count)
    axle_names = _axle_names(len(axles))

    rows = []
    for axle_name, axle in zip(axle_names, axles):
        # Positive lateral is LEFT, so the left end reads first.
        ends = (
            ("left", [w for w in axle if w["lat"] > CENTRE_LAT_M]),
            ("", [w for w in axle if -CENTRE_LAT_M <= w["lat"] <= CENTRE_LAT_M]),
            ("right", [w for w in axle if w["lat"] < -CENTRE_LAT_M]),
        )
        for side, group in ends:
            if not group:
                continue
            group.sort(key=lambda w: abs(w["lat"]))
            stack = _stack_names(len(group))
            # Read outward on the left and inward on the right: left to right across the axle.
            order = list(zip(stack, group))
            if side == "left":
                order.reverse()
            for stack_name, w in order:
                dual = (
                    vehicle["dual"]
                    and narrowest > 0
                    and w["width"] >= narrowest * DUAL_WIDTH_RATIO
                )
                words = [prefix, axle_name, side, stack_name, "dual" if dual else ""]
                label = " ".join(word for word in words if word) or "wheel"
                rows.append(
                    {
                        "label": label,
                        "psi": w["psi"],
                        "brake_c": w["brake_c"],
                        "broken": w["broken"],
                    }
                )
    return rows


def all_rows(vehicles):
    """Rows for the driven vehicle, then each coupled vehicle in the order the mod sent them."""
    vehicles = sorted(vehicles or [], key=lambda v: v["role"])
    trailers = sum(1 for v in vehicles if v["role"] > 0)
    rows = []
    for v in vehicles:
        rows.extend(vehicle_rows(v, trailers))
    return rows
