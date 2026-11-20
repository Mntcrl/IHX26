#!/usr/bin/env python3
"""Offline, experimental multi-frame consistency check. Never authenticates origin."""
import argparse
import json
from pathlib import Path

import cv2 as cv
import numpy as np


def related(folder, filename):
    if not isinstance(filename, str) or Path(filename).name != filename or filename in {".", ".."}:
        raise ValueError("Unsafe evidence filename")
    return folder / filename


def load_json(path):
    return json.loads(path.read_bytes())


def luma(path, width, height, bits, offset=0):
    if bits not in (8, 10) or not (1 <= width <= 8192 and 1 <= height <= 8192):
        raise ValueError("Unsupported luma format")
    pixels = np.fromfile(path, dtype=np.uint8 if bits == 8 else "<u2", count=width * height, offset=offset)
    if bits == 10:
        pixels = (pixels >> 8).astype(np.uint8)
    return pixels.reshape(height, width)


def depth(path):
    with path.open("rb") as file:
        header = file.read(12)
        if len(header) != 12 or header[:4] != b"FLDP":
            raise ValueError("Invalid depth file")
        width, height = np.frombuffer(header[4:], "<u4").tolist()
        if not (1 <= width <= 4096 and 1 <= height <= 4096):
            raise ValueError("Invalid depth resolution")
        return np.fromfile(file, "<f4").reshape(height, width)


def scale_intrinsics(k, width_ratio, height_ratio):
    # Account for OpenCV's pixel-centered resize.
    warp = np.array([[width_ratio, 0, (width_ratio - 1) / 2],
                     [0, height_ratio, (height_ratio - 1) / 2], [0, 0, 1]])
    return warp @ np.asarray(k, np.float64).reshape(3, 3)


def sample_z(points, depth_map, width, height):
    dh, dw = depth_map.shape
    xy = np.floor((points + 0.5) * [dw / width, dh / height]).astype(int)
    valid = (xy[:, 0] >= 0) & (xy[:, 1] >= 0) & (xy[:, 0] < dw) & (xy[:, 1] < dh)
    z = np.full(len(points), np.nan)
    z[valid] = depth_map[xy[valid, 1], xy[valid, 0]]
    z[(z < 0.12) | (~np.isfinite(z))] = np.nan
    return z


def gyro_rotation(gyro, start, end):
    if gyro.ndim != 2 or gyro.shape[1] != 4 or len(gyro) < 3 or not np.isfinite(gyro).all():
        return None
    a, b = sorted((start, end))
    times = gyro[:, 0]
    if np.any(np.diff(times) <= 0) or a < times[0] or b > times[-1] or b <= a:
        return None
    start_idx, end_idx = np.searchsorted(times, [a, b])
    if np.diff(times[max(0, start_idx - 1):end_idx + 1]).max(initial=0) > 0.04:
        return None
    t = np.r_[a, times[(times > a) & (times < b)], b]
    angular = np.column_stack([np.interp(t, times, gyro[:, i]) for i in (1, 2, 3)])
    orientation = np.eye(3)
    for rate, dt in zip((angular[:-1] + angular[1:]) / 2, np.diff(t)):
        orientation = orientation @ cv.Rodrigues(rate * dt)[0]
    # An angle is invariant under the unknown fixed camera/device basis rotation.
    return float(np.degrees(np.linalg.norm(cv.Rodrigues(orientation)[0])))


def compare(shutter, reference, k0, k1, z0, z1, gyro_degrees):
    height, width = shutter.shape
    result = {"status": "insufficientEvidence", "gyroRotationDegrees": gyro_degrees}
    features = cv.goodFeaturesToTrack(shutter, 500, 0.015, 8)
    if features is None or len(features) < 36:
        return result | {"reason": "tooFewImageFeatures"}
    forward, ok1, _ = cv.calcOpticalFlowPyrLK(shutter, reference, features, None)
    if forward is None:
        return result | {"reason": "trackingUnavailable"}
    backward, ok2, _ = cv.calcOpticalFlowPyrLK(reference, shutter, forward, None)
    if backward is None:
        return result | {"reason": "trackingUnavailable"}
    p = features.reshape(-1, 2)
    q = forward.reshape(-1, 2)
    tracked = (ok1.ravel() == 1) & (ok2.ravel() == 1)
    tracked &= np.linalg.norm(backward.reshape(-1, 2) - p, axis=1) < 1.5
    tracked &= np.isfinite(q).all(axis=1) & (q >= 0).all(axis=1) & (q < [width, height]).all(axis=1)
    z = sample_z(p, z0, width, height)
    use = tracked & np.isfinite(z)
    p, q, z = p[use], q[use], z[use]
    result["validTracks"] = len(p)
    if len(p) < 36 or gyro_degrees is None or z1 is None:
        return result | {"reason": "missingTracksGyroOrNeighborDepth"}
    if np.linalg.norm(np.ptp(p, axis=0) / [width, height]) < 0.5 or np.quantile(z, .9) - np.quantile(z, .1) < .1:
        return result | {"reason": "featuresTooLocalizedOrDepthTooUniform"}

    rays = np.c_[p, np.ones(len(p))] @ np.linalg.inv(k0).T
    points = np.ascontiguousarray(rays * z[:, None])
    train = np.arange(len(p)) % 4 != 0
    ok, rvec, tvec, inliers = cv.solvePnPRansac(points[train], q[train], k1, None, iterationsCount=150,
                                                reprojectionError=2.5, flags=cv.SOLVEPNP_EPNP)
    if not ok or inliers is None or len(inliers) < 18 or len(inliers) < .55 * train.sum():
        return result | {"reason": "noStableRigidFit"}

    t = float(np.linalg.norm(tvec))
    visual_angle = float(np.degrees(np.linalg.norm(rvec)))
    rotation = cv.Rodrigues(rvec)[0]
    test = ~train
    rotate_only = (points[test] @ rotation.T) @ k1.T
    if np.any(rotate_only[:, 2] <= 0):
        return result | {"reason": "invalidPoseProjection"}
    parallax = float(np.median(np.linalg.norm(q[test] - rotate_only[:, :2] / rotate_only[:, 2:], axis=1)))
    result |= {"translationMeters": t, "visualRotationDegrees": visual_angle,
               "rotationDisagreementDegrees": abs(visual_angle - gyro_degrees), "medianParallaxPixels": parallax}
    if t < .006 or parallax < 1.0:
        return result | {"reason": "tooLittleTranslationalParallax"}

    predicted = points[test] @ rotation.T + tvec.ravel()
    projected = cv.projectPoints(points[test], rvec, tvec, k1, None)[0].reshape(-1, 2)
    rm = float(np.median(np.linalg.norm(projected - q[test], axis=1)))
    observed = sample_z(q[test], z1, width, height)
    good = np.isfinite(observed) & (predicted[:, 2] > 0)
    result |= {"heldOutMedianReprojectionPixels": rm, "heldOutDepthPairs": int(good.sum())}
    if good.sum() < 8:
        return result | {"reason": "tooFewHeldOutDepthPairs"}
    depth_error = float(np.median(np.abs(predicted[good, 2] - observed[good]) / observed[good]))
    result["heldOutMedianRelativeDepthError"] = depth_error
    passed = rm <= 3 and depth_error <= 0.12 and result["rotationDisagreementDegrees"] <= 2.5
    return result | {"status": "physicallyConsistent" if passed else "inconsistent"}


def verify(path):
    folder = path.parent
    burst = load_json(path)
    if burst["schemaVersion"] != 1:
        raise ValueError("Unsupported physics manifest")
    shutter = load_json(related(folder, burst["shutterManifest"]))
    native = shutter.get("cameraEvidence", {}).get("nativeFrame", {})
    if "file" not in native or not shutter.get("depth") or not burst.get("references"):
        return {"status": "insufficientEvidence", "reason": "missingNativeFrameDepthOrReferences"}
    plane = native["planes"][0]
    full = luma(related(folder, native["file"]), plane["width"], plane["height"],
                native["bitsPerComponent"], plane["offsetBytes"])
    height, width = full.shape
    size = (min(960, width), round(height * min(960, width) / width))
    source = cv.resize(full, size, interpolation=cv.INTER_AREA)
    k0 = scale_intrinsics(shutter["camera"]["intrinsics"], size[0] / width, size[1] / height)
    z0 = depth(related(folder, shutter["depth"]["file"]))
    imu = load_json(related(folder, burst["imuFile"]))
    gyro = np.asarray(imu["gyroscope"]["samples"], np.float64)
    start = burst["shutterOffsetSecondsFromCaptureWindowCenter"]
    comparisons = []
    for frame in burst["references"][:2]:
        end = frame["offsetSecondsFromCaptureWindowCenter"]
        if not isinstance(start, (int, float)) or not isinstance(end, (int, float)):
            comparisons.append({"status": "insufficientEvidence", "reason": "missingFrameTimestamp"})
            continue
        data = luma(related(folder, frame["lumaFile"]), frame["width"], frame["height"], frame["bitsPerComponent"])
        k1 = scale_intrinsics(frame["intrinsics"], size[0] / data.shape[1], size[1] / data.shape[0])
        neighbor = cv.resize(data, size, interpolation=cv.INTER_AREA)
        z1 = depth(related(folder, frame["depthFile"])) if frame.get("depthFile") else None
        comparison = compare(source, neighbor, k0, k1, z0, z1, gyro_rotation(gyro, start, end))
        comparisons.append(comparison | {"offsetSeconds": end - start})
    statuses = [item["status"] for item in comparisons]
    status = ("inconsistent" if "inconsistent" in statuses else
              "physicallyConsistent" if "physicallyConsistent" in statuses else "insufficientEvidence")
    return {"status": status, "comparisons": comparisons,
            "scope": "Heuristic geometry and gyro consistency only. No authentication of sensor origin."}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("physics_json", type=Path)
    print(json.dumps(verify(parser.parse_args().physics_json), indent=2, allow_nan=False))
