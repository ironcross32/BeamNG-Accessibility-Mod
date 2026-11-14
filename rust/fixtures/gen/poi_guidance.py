"""Golden cases for beam_reports::poi_guidance, from poi_guidance.py."""

from __future__ import annotations

import json
import random

from _common import fbits, write_fixture

import poi_guidance as pg


def poi_out(p):
    return {"name": p["name"], "description": p["description"], "distance": fbits(p["distance"]),
            "bearing": fbits(p["bearing"]), "radius": fbits(p["radius"])}


def main():
    rnd = random.Random(2596)
    texts = [
        "Foot &amp; start<br/>Asphalt", "<BR>", "a< br / >b", "a<bR  >b", "a<br/ >b", "a< b r>b",
        "<b>bold</b> text", "x<y", "x>y", "<<a>>", "a <br> . b", "...dots...", " . ", ".",
        "tabs\tand\nnewlines", " nbsp em ", "&lt;b&gt; escaped", "&amp;lt;", "&notin; &noti",
        "&#60;br&#62;", "<p>para</p><p>two</p>", "<a href='x'>link</a>", "<br/><br/>", "",
        "unterminated <tag", "\u0085x y", "&nbsp;&nbsp;x", "< br>", "<br　/>", "<ＢＲ>",
        "Hill Climb - Base", "A&B", "&#x1F600; smile", "&",
    ]
    spoken = [{"in": t, "out": pg._spoken_text(t)} for t in texts]

    packets = [
        "SCAN,1", "POI:", "POI:not-json", "POI:[]", "POI:null", 'POI:{"name":"Broken"}',
        'POI:{"distance":-1,"bearing":0}', 'POI:{"distance":0,"bearing":0}',
        'POI:{"distance":"5","bearing":"x"}', 'POI:{"distance":null,"bearing":0}',
        'POI:{"distance":NaN,"bearing":0}', 'POI:{"distance":1,"bearing":Infinity}',
        'POI:{"distance":1,"bearing":0,"radius":"r"}', 'POI:{"distance":1,"bearing":0,"radius":-3}',
        'POI:{"distance":1,"bearing":0,"radius":null}', 'POI:{"distance":1e400,"bearing":0}',
        'POI:{"distance":1' + "0" * 400 + ',"bearing":0}', 'POI:{"distance":-0.0,"bearing":-0.0}',
        'POI:{"name":5,"description":["x"],"distance":3,"bearing":540}', 'poi:{"distance":1,"bearing":0}',
        'POI:{"name":"Hill Climb - Base","description":"Foot &amp; start<br/>Asphalt","distance":3.2,"bearing":91,"radius":0}',
        'POI:{"name":"Garage","description":"Repairs","distance":108,"bearing":-27.4,"radius":8}',
        'POI:{"name":"Fuel","distance":18,"bearing":0.2,"radius":0}', 'POI:{"name":"","distance":1,"bearing":true}',
        'POI:{"name":"<br>","distance":1,"bearing":0}',
    ]
    for _ in range(60):
        row = {"name": rnd.choice(["Garage", "", "<b>X</b>"]), "distance": rnd.choice(
            [0.0, 7.9, 8.0, 8.1, 8.15, 8.16, 50.0, rnd.uniform(0, 5000)]),
               "bearing": rnd.choice([0.0, 0.49, 0.5, -0.5, -0.49, 179.5, 180.0, -180.0, 270.0,
                                      rnd.uniform(-720, 720)])}
        if rnd.random() < 0.6:
            row["radius"] = rnd.choice([0, 7.9, 8.0, 12.5, 30.0, -1.0])
        if rnd.random() < 0.5:
            row["description"] = rnd.choice(["", "Repairs", "a<br>b"])
        packets.append("POI:" + json.dumps(row))
    out = []
    for p in packets:
        try:
            poi = pg.parse_poi_packet(p)
        except Exception as exc:  # noqa: BLE001
            out.append({"packet": p, "err": type(exc).__name__, "msg": str(exc)})
            continue
        out.append({"packet": p, "ok": poi_out(poi),
                    "phrases": {u: pg.poi_phrase(poi, u) for u in ("imperial", "metric", "Imperial")}})

    write_fixture("poi_guidance", {
        "constants": {"POI_PACKET_PREFIX": pg.POI_PACKET_PREFIX,
                      "POI_PARKED_RADIUS_M": fbits(pg.POI_PARKED_RADIUS_M)},
        "spoken_text": spoken,
        "packets": out,
    })


if __name__ == "__main__":
    main()
