#!/usr/bin/env python3
"""Recompute every residual and validate motion timestamps for an unpacked scan."""
from array import array
import json
import math
from pathlib import Path
import sys


def read_json(path):
    return json.loads(path.read_bytes())


def int16(data, typecode):
    values = array(typecode)
    values.frombytes(data)
    if sys.byteorder != "little":
        values.byteswap()
    return values


def verify(noise_path):
    stem = noise_path.name.removesuffix(".noise.json")
    if not noise_path.name.endswith(".noise.json"):
        raise ValueError("Expected <scan>.noise.json")
    folder = noise_path.parent
    noise = read_json(noise_path)
    native = read_json(folder / f"{stem}.json")["cameraEvidence"]["nativeFrame"]
    assert noise["sourceNativeFrame"] == native["file"] == f"{stem}.yuv"
    assert noise["residualFile"] == f"{stem}.noise.i16"
    raw = (folder / native["file"]).read_bytes()
    residuals = int16((folder / noise["residualFile"]).read_bytes(), "h")
    bits = native["bitsPerComponent"]
    assert bits in (8, 10)
    sample_bytes = 1 if bits == 8 else 2
    input_planes = {
        channel: (plane, index)
        for plane in native["planes"]
        for index, channel in enumerate(plane["channels"])
    }
    values = raw if bits == 8 else int16(raw, "H")
    checked = 0
    for output in noise["spatialChannels"]:
        name = output["channel"]
        plane, index = input_planes[name]
        width, height = plane["width"], plane["height"]
        assert [output["width"], output["height"]] == [width, height]
        assert output["offsetBytes"] == 2 * checked and output["rowBytes"] == 2 * width
        stride = plane["rowBytes"] // sample_bytes
        channels = len(plane["channels"])
        start = plane["offsetBytes"] // sample_bytes + index
        decoded = [
            [values[start + y * stride + x * channels] >> (6 if bits == 10 else 0) for x in range(width)]
            for y in range(height)
        ]
        squared = 0
        for y, row in enumerate(decoded):
            above = decoded[max(0, y - 1)]
            below = decoded[min(height - 1, y + 1)]
            for x, center in enumerate(row):
                expected = 4 * center - row[max(0, x - 1)] - row[min(width - 1, x + 1)] - above[x] - below[x]
                if expected != residuals[checked]:
                    raise ValueError(f"{name} residual mismatch at ({x}, {y})")
                checked += 1
                squared += expected * expected
        assert math.isclose(output["rms"], math.sqrt(squared / (width * height)), abs_tol=1e-9)
    assert checked == len(residuals)

    session = read_json(folder / f"{stem}.session.json")
    default_half = session["observationWindow"] / 2000
    start = -session.get("observationPreMilliseconds", default_half * 1000) / 1000
    end = session.get("observationPostMilliseconds", default_half * 1000) / 1000
    imu = read_json(folder / f"{stem}.imu.json")
    for sensor in ("accelerometer", "gyroscope"):
        series = imu[sensor]
        samples = series["samples"]
        assert series["sampleCount"] == len(samples)
        previous = -math.inf
        for row in samples:
            assert len(row) == 4 and all(math.isfinite(v) for v in row)
            offset = row[0]
            assert start <= offset <= end and previous < offset
            previous = offset
        if samples:
            assert math.isclose(series["observedSpanSeconds"], samples[-1][0] - samples[0][0], abs_tol=1e-8)
        print(f"{sensor}: {len(samples)} samples, {series['availability']}, {series.get('observedSamplesPerSecond')} Hz")
    print(f"Compared all {checked:,} spatial residual samples.")
    print("Temporal comparisons:", [item["status"] for item in noise["temporalComparisons"]])


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("Usage: check-camera-evidence.py /path/to/<scan>.noise.json")
    verify(Path(sys.argv[1]))
