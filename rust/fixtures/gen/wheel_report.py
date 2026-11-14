"""Golden cases for beam_reports::wheel_report, from wheel_report.py."""

from __future__ import annotations

import random

from _common import write_fixture
from _pyvalue import enc

import wheel_report as wr


def wheel(lon, lat, width=0.265, psi="90", brake="60", broken="0"):
    return f"{lon},{lat},{width},{psi},{brake},{broken}"


def main():
    rnd = random.Random(4485)
    packets = [
        "", "WHEELS", " WHEELS \n", "wheels", "TRAILER:CLEAR", "WHEELS\nV,0,5,Prop\n0|",
        "WHEELS\nV,0,5,Prop", "WHEELS\nV,0,5", "WHEELS\nV,0", "WHEELS\nV,x,5,P\n0|1,1,1,1,1,0",
        "WHEELS\nV, 1 ,+7,Name, with, commas\n1|1,1,0.3,,,0;1,-1,0.3,32,,1",
        "WHEELS\nV,0,1,A\nV,1,2,B\n0|1,1,1,1,1,0", "WHEELS\nnoise\nV,0,1,A\n0|1,1,1,1,1,0\njunk",
        "WHEELS\r\nV,0,1,A\r\n0|1,0.8,0.2,30,40,0;1,-0.8,0.2,30,40,0\r\n",
        "WHEELS\nV,0,1,A\n0|1,1,1,1,1",  # short wheel
        "WHEELS\nV,0,1,A\n0|a,1,1,1,1,0;1,1,1,1,1,0", "WHEELS\nV,0,1,A\n0|1,1,1,x,1,0",
        "WHEELS\nV,0,1,A\n0|nan,0,0.2,,,0;1,nan,0.2,,,0;-1,0.5,0.2,,,0",
        "WHEELS\nV,0,1,A\n0|1,0.8,0.2,1_0,inf,0;1_0,-0.8,0.2,-0.0,,1",
        "WHEELS\nV,0,1,A\n1|1,0.8,0,,,0;1,-0.8,-1,,,0", "WHEELS\nV,0,1,A\n0|1,1,1,1,1,0,extra",
        "WHEELS\nV,0,1\n0|1,0.2,1,1,1,0;1,-0.2,1,1,1,0;1,0.2000001,1,1,1,0",
        "WHEELS\nV,0,99999999999999999999999,A\n0|1,1,1,1,1,0", "WHEELS\nV,٢,1,A\n0|١,1,1,1,1,0",
        "WHEELS\nV,0,1,A\n0", "WHEELS\nV,0,1,A\n|1,1,1,1,1,0", "WHEELS\nV,0,1,A\n 1 |1,1,1,1,1, 1 ",
    ]
    # The sim's shapes, plus random vehicles.
    for _ in range(120):
        lines = ["WHEELS"]
        for role in rnd.sample(range(0, 4), rnd.randint(1, 4)):
            lines.append(f"V,{role},{1000 + role},Vehicle {role}")
            ws_ = []
            n_axles = rnd.randint(1, 5)
            for a in range(n_axles):
                lon = round(3.0 - a * rnd.choice([0.3, 0.5, 0.51, 1.3, 2.0]), 3)
                for side in rnd.choice([[1], [-1, 1], [-1, 1, -1, 1], [0], [-1, 1, -1, 1, -1, 1, 1, -1]]):
                    lat = round(side * rnd.uniform(0.0, 1.2) + rnd.choice([0.0, 0.0, 0.7]), 3)
                    width = rnd.choice([0.265, 0.59, 0.2, 0.0, 0.43])
                    psi = rnd.choice(["", "0", "32.5", "90"])
                    brake = rnd.choice(["", "0", "120"])
                    ws_.append(wheel(lon, lat, width, psi, brake, rnd.choice(["0", "1"])))
            rnd.shuffle(ws_)
            lines.append(f"{rnd.choice(['0', '1'])}|" + ";".join(ws_))
        packets.append("\n".join(lines))

    out = []
    for text in packets:
        parsed = wr.parse_packet(text)
        case = {"text": text, "parsed": enc(parsed)}
        if parsed is not None:
            case["all_rows"] = enc(wr.all_rows(parsed))
            case["vehicle_rows"] = [
                {"trailer_count": tc, "rows": enc(wr.vehicle_rows(v, tc))}
                for v in parsed for tc in (0, 1, 2)
            ]
        out.append(case)
    write_fixture("wheel_report", {
        "constants": {"AXLE_GAP_M": wr.AXLE_GAP_M, "CENTRE_LAT_M": wr.CENTRE_LAT_M,
                      "DUAL_WIDTH_RATIO": wr.DUAL_WIDTH_RATIO},
        "all_rows_none": enc(wr.all_rows(None)),
        "packets": out,
    })


if __name__ == "__main__":
    main()
