<p align="center">
  <img src="packaging/brand/gamelo-512.png" width="128" alt="Gamelo icon">
</p>

<h1 align="center">Gamelo</h1>
<p align="center"><b>Your games, in one place.</b><br>
A game launcher for Linux that runs your Windows games through Proton — no Steam client needed.</p>

---

Gamelo is a native desktop app (Rust + egui) that keeps your whole game collection in one library: games you installed yourself, your Epic Games and GOG libraries, games from Windows launchers like Battle.net, and your ROMs. Windows games run through [umu-launcher](https://github.com/Open-Wine-Components/umu-launcher) and [GE-Proton](https://github.com/GloriousEggroll/proton-ge-custom), each in its own prefix.

## Features

- **Library** — a poster grid (three tile sizes) or a list, with favourites, statuses (Playing, Backlog, Finished…), collections, search and sorting. Artwork comes from [SteamGridDB](https://www.steamgriddb.com) automatically.
- **Home** — jump back into your last game, this week at a glance, a "not sure what to play?" pick from your backlog, recent games, and what's new in your stores.
- **Stats** — playtime by week, month and year, a daily heatmap, streaks, most played, and monthly and yearly recaps.
- **Game pages** — playtime, HowLongToBeat times, ProtonDB rating, Steam store info and screenshots, save backups, and per-game launch options.
- **Stores** — sign in to **Epic Games** and **GOG** (through [legendary](https://github.com/derrod/legendary) and [gogdl](https://github.com/Heroic-Games-Launcher/heroic-gogdl), downloaded on demand) to install, update and play your games, with Epic cloud saves. Games installed with Heroic can be imported.
- **Launchers** — install **Battle.net**, the **EA app**, **Ubisoft Connect** or the **Rockstar Games Launcher** into their own prefix; games you install through them show up in your library.
- **Emulation** — add ROM folders and games are sorted onto shelves by console, run with the emulators you already have (RetroArch or standalone, native or Flatpak).
- **Tuning** — MangoHud presets, an FPS limit, gamescope, GameMode, FSR, OptiScaler, winetricks components.
- **Proton manager** — download GE-Proton builds and see which games use which.
- **Saves** — automatic backups before and after each session, optional sync folder, restore any backup.
- **Couch mode** — a fullscreen, controller-friendly carousel (Guide button or F11). Gamepads work everywhere in the app.
- **Quick search** — `Ctrl+K`, type a few letters, `Enter` to play.
- **Tray icon** and a **global shortcut** (`Meta+G`, through the desktop's shortcut portal).
- **Disk space** page to find and free what Gamelo's prefixes, caches and old Proton builds take up.
- **Imports** — scan a folder for games, bring over non-Steam games from Steam, or games from Lutris.
- **Themes** — Gamelo Night/Day and several others, accent colours, rounded or square corners, UI scale.
- **Arabic** — a full right-to-left Arabic translation, alongside English.

## Install

Grab `Gamelo-x86_64.AppImage` from the [releases](https://github.com/barknq11/gamelo/releases) (when available), make it executable and run it:

```sh
chmod +x Gamelo-x86_64.AppImage
./Gamelo-x86_64.AppImage
```

On first start Gamelo checks for **umu-run** and **GE-Proton** and can install both into your home folder (no root needed).

## Build from source

```sh
cargo build --release
./target/release/gamelo
```

To build the portable AppImage (runs on glibc 2.17+ distros), install [zig](https://ziglang.org/download/), `cargo install cargo-zigbuild` and [appimagetool](https://github.com/AppImage/appimagetool), then:

```sh
./packaging/build-appimage.sh
```

Tests, including offscreen screenshot suites (`SHOTS=<dir> cargo test -- --ignored <suite>`):

```sh
cargo test
```

## Optional extras

- A free [SteamGridDB API key](https://www.steamgriddb.com/profile/preferences/api) (Settings) for automatic artwork.
- [MangoHud](https://github.com/flightlessmango/MangoHud), [GameMode](https://github.com/FeralInteractive/gamemode) and [gamescope](https://github.com/ValveSoftware/gamescope) for the tuning options.

## Where things live

| | |
|---|---|
| Settings and library | `~/.config/gamelo/config.json` (backed up daily to `~/.local/share/gamelo/backups`) |
| Prefixes, artwork, logs, save backups | `~/.local/share/gamelo/` |
| Caches | `~/.cache/gamelo/` |

## Notes

Gamelo isn't affiliated with Valve, Epic Games, GOG, Blizzard, Electronic Arts, Ubisoft, Rockstar Games or SteamGridDB. It doesn't bypass DRM: store and launcher games need your own account, and games with kernel-level anti-cheat generally don't run on Linux.

## License

[MIT](LICENSE)
