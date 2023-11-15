# Clunky

A mix of [conky](https://github.com/brndnmtthws/conky) & [Rainmeter](https://www.rainmeter.net/). If you're looking for something functional, take a look at
conky.

This is (at least for now) a toy project I use for practicing Wayland, adding configurability through Lua, and many other smaller things I haven't done properly before. Worst outcome is it being archived once I achieve everything new I wanted to try out, best outcome is a good Rainmeter alternative that's cross platform.

I also want to experiment with allowing scripts to define custom components with custom behaviors so that new additions and very specific capabilities (e.g. RSS, iTunes, ...) don't need to be added directly to the sources.

## Widgets

Some components are expensive to redraw without Clunky keeping track

## Build

### Linux

#### wlroots

To build Clunky for wlroots based compositors (Sway, Wayfire, Hyprland, ...) use the `wlr` feature:

```sh
cargo build --release --features wlr
```

## Functionality

This program is still in early phases of development. There's key parts of
intended functionality still missing and a lot of things are bound to change by
the time it's deemed feature complete.

## Contributions

Don't contribute here and instead contribute to [conky](https://github.com/brndnmtthws/conky) which is an already finished version of this program with a bigger and more active user base.

## License

This program, resulting binaries and resources used by this program are all licensed under GPLv3 where applicable. A copy of the license text can be found
in the [LICENSE](./LICENSE) file in the root of this repository.

Script licenses are up to user discretion.
