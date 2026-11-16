#!/usr/bin/env python3
"""Génère uniquement les fichiers de démonstration connus, sans dépendance externe."""

from pathlib import Path
import struct
import xml.etree.ElementTree as ET
import zlib


ROOT = Path(__file__).resolve().parent
GAMES = (
    ("nes", "Pixel Pilot", "Pixel Pilot.nes", (47, 184, 162)),
    ("nes", "L'île & les étoiles", "L'île & les étoiles.nes", (238, 169, 65)),
    ("snes", "Turbo Garden", "Turbo Garden.sfc", (166, 112, 225)),
    ("snes", "Sans image", "Sans image.sfc", None),
    ("psx", "Orbital Quest", "Orbital Quest.m3u", (69, 150, 236)),
)


def write(relative, data):
    path = ROOT / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


def png(width, height, color):
    """Image RGB valide : paysage pixel-art, sans contenu tiers."""
    rows = bytearray()
    for y in range(height):
        rows.append(0)  # Filtre PNG : aucun.
        for x in range(width):
            u, v = x * 64 // width, y * 64 // height
            pixel = (17, 26, 48)
            if (u * 13 + v * 7) % 97 == 0:
                pixel = (218, 234, 250)
            if (u - 46) ** 2 + (v - 16) ** 2 < 64:
                pixel = color
            if v > 49 - abs(u - 20) // 2:
                pixel = tuple(channel // 2 for channel in color)
            if v > 60:
                pixel = color
            rows.extend(pixel)

    def chunk(kind, data):
        checksum = zlib.crc32(kind + data) & 0xFFFFFFFF
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", checksum)

    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(bytes(rows)))
        + chunk(b"IEND", b"")
    )


def main():
    systems = {}
    for system, name, filename, color in GAMES:
        base = Path("library") / system
        if filename.endswith(".m3u"):
            discs = [f"{name}/{name} (Disc {disc}).chd" for disc in (1, 2)]
            write(base / filename, ("\n".join(discs) + "\n").encode("utf-8"))
            for disc in discs:
                write(base / disc, f"FAKE ROM - NOT PLAYABLE\n{disc}\n".encode("utf-8"))
        else:
            write(base / filename, f"FAKE ROM - NOT PLAYABLE\n{name}\n".encode("utf-8"))

        game = ET.SubElement(systems.setdefault(system, ET.Element("gameList")), "game")
        metadata = {
            "path": f"./{filename}",
            "name": name,
            "desc": f"{name} : jeu fictif de démonstration, non jouable. "
                    "Permet de tester le catalogue, les téléchargements et le XML.",
            "developer": "ROM Store Demo",
            "publisher": "ROM Store Demo",
            "genre": "Demo",
            "players": "1-2",
            "rating": "0.8",
            "releasedate": "20000101T000000",
        }
        if color is not None:
            for tag, suffix, size in (
                ("image", "image", (480, 270)),
                ("thumbnail", "thumb", (240, 320)),
            ):
                image = f"images/{name}-{suffix}.png"
                write(base / image, png(*size, color))
                metadata[tag] = f"./{image}"
        for tag, value in metadata.items():
            ET.SubElement(game, tag).text = value

    for (system, gamelist), color in zip(
        systems.items(), ((47, 184, 162), (166, 112, 225), (69, 150, 236))
    ):
        ET.indent(gamelist, space="  ")
        write(
            Path("library") / system / "gamelist.xml",
            ET.tostring(gamelist, encoding="utf-8", xml_declaration=True) + b"\n",
        )
        write(Path("system-icons") / f"{system}.png", png(256, 256, color))
    print("Bibliothèque synthétique générée : 3 plateformes, 5 jeux, 11 images PNG.")


if __name__ == "__main__":
    main()
