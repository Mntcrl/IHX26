"""Prepare private development image variants and a development-only trust config."""

import argparse
import json
from pathlib import Path
import re
import tomllib


def configured_circuit(args):
    config = tomllib.loads(args.base.read_text())
    if config["network_id"] != 1 or config["allow_mainnet"]:
        raise SystemExit("development: expected an XRPL Testnet configuration")
    circuit_id = args.id_file.read_text().strip()
    matches = [entry for entry in config["accepted_circuits"] if entry["circuit_id"] == circuit_id]
    if len(matches) != 1:
        raise SystemExit("development: the artifact circuit must have exactly one explicitly accepted registration")
    print(json.dumps({"mode": "development", "registry": "circuit", **matches[0]}))


def network(args):
    config = tomllib.loads(args.base.read_text())
    if config["network_id"] != 1 or config["allow_mainnet"]:
        raise SystemExit("development: expected an XRPL Testnet configuration")
    config.pop("accepted_circuits")
    if args.inception is not None:
        config["inception_ledger"] = args.inception
    registration = json.loads(args.circuit.read_text())
    keys = ("circuit_id", "policy_id", "vk_digest", "registration_tx")
    if registration["mode"] != "development" or registration["registry"] != "circuit" or any(
        not re.fullmatch(r"[0-9a-fA-F]{64}", registration[key]) for key in keys
    ):
        raise SystemExit("development: expected a validated development CIRCUIT registration report")
    print("# Development Testnet only. The operator explicitly selected the local circuit bundle.")
    for key, value in config.items():
        print(f"{key} = {json.dumps(value)}")
    if args.unrecognized:
        print("accepted_circuits = []")
    else:
        print("\n[[accepted_circuits]]")
        for key in keys:
            print(f"{key} = {json.dumps(registration[key])}")


def images(args):
    from PIL import Image, PngImagePlugin

    keyword = "XML:com.adobe.xmp"
    args.output.mkdir(mode=0o700, exist_ok=True)

    def save_png(image, name, xmp=None):
        attrs = PngImagePlugin.PngInfo()
        if xmp is not None:
            attrs.add_itxt(keyword, xmp)
        image.save(args.output / (name + ".png"), pnginfo=attrs)

    with Image.open(args.png) as png, Image.open(args.jpeg_raster_png) as other, Image.open(args.jpeg) as jpeg:
        xmp = png.info[keyword]
        other_xmp = other.info[keyword]
        jpg_xmp = jpeg.info["xmp"]
        assert png.mode == "RGB" and png.size == (1024, 768) and xmp != other_xmp
        save_png(png, "metadata-removed")
        save_png(other, "metadata-removed-jpeg-raster")
        save_png(png, "metadata-substituted", other_xmp)
        pixel = png.copy()
        r, g, b = pixel.getpixel((2, 2))
        pixel.putpixel((2, 2), (r ^ 1, g, b))
        save_png(pixel, "one-pixel", xmp)
        save_png(png.crop((1, 1, 1024, 768)), "crop", xmp)
        save_png(png.resize((768, 576), Image.Resampling.BICUBIC), "resize", xmp)
        jpeg.save(args.output / "recompressed.jpg", quality=55, subsampling=0, xmp=jpg_xmp)
        save_png(Image.new("RGB", (1024, 768), (37, 105, 193)), "unregistered")


parser = argparse.ArgumentParser(description=__doc__)
commands = parser.add_subparsers(required=True)
configured = commands.add_parser("configured-circuit")
configured.add_argument("base", type=Path)
configured.add_argument("id_file", type=Path)
configured.set_defaults(run=configured_circuit)
config = commands.add_parser("network")
config.add_argument("base", type=Path)
config.add_argument("circuit", type=Path)
config.add_argument("--inception", type=int)
config.add_argument("--unrecognized", action="store_true")
config.set_defaults(run=network)
variants = commands.add_parser("images")
variants.add_argument("png", type=Path)
variants.add_argument("jpeg", type=Path)
variants.add_argument("jpeg_raster_png", type=Path)
variants.add_argument("output", type=Path)
variants.set_defaults(run=images)
args = parser.parse_args()
args.run(args)
