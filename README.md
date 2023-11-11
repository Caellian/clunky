# Clunky

A mix of [conky](https://github.com/brndnmtthws/conky) & [Rainmeter](https://www.rainmeter.net/).

This is (at least for now) a toy project I use for practicing Wayland, adding configurability through Lua, and many other smaller things I haven't done properly before. Worst outcome is it being archived once I achieve everything new I wanted to try out, best outcome is a good Rainmeter alternative that's cross platform.

I also want to experiment with allowing scripts to define custom components with custom behaviors so that new additions and very specific capabilities (e.g. RSS, iTunes, ...) don't need to be added directly to the sources.

## Widgets

Some components are expensive to redraw without Clunky keeping track

## Build
### Linux
#### wlroots

To build Clunky for wlroots based compositors (Sway, Wayfire, ...) use the `wlr` feature:

```sh
cargo build --release --features wlr
```

## Functionality

- [x] Allow running lua scripts
- [x] Add components
  - [x] Label
  - [x] Button
  - [ ] Image
    - [ ] ImageButton
- [ ] Wayland support
  - [x] Window creation
  - [x] wlr-layer-shell support
  - [ ] KWin support
- [ ] Process forking
- [ ] Recreate _some_ conky functionality
  - CPU load & temp., memory usage, uptime / boot time, battery life, FS mounts, disk usage, disk IO, network, network traffic stats
- [ ] Actions
  - [ ] Running applications & scripts
- [ ] Allow running custom shaders for surfaces
  - [ ] Background mode
- [ ] Windows support
  - [ ] Keep Windows window in background
    - https://stackoverflow.com/questions/49396096/set-window-z-order-above-other

## Contributions

Don't contribute here (unless you really, really want to) and instead contribute to [conky](https://github.com/brndnmtthws/conky) which already has most of the features implemented by this program and a bigger and more active user base.

## License

This program, resulting binaries and resources used by this program are all licensed under GPLv3 where applicable.

Script licenses are up to user discretion.
