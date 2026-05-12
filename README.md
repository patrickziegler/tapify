# :vhs: taped

> [!NOTE]
> Not ready yet! There is a major rewrite of this project going on at the moment.
>
> The codebase will be ported to [Rust](https://rust-lang.org/) and use [PipeWire](https://www.pipewire.org/) for recording audio.
>
> The last release of the Python based version was [v1.1.4](https://github.com/patrickziegler/taped/releases/tag/v1.1.4).

## Development

```sh
podman run -it \
    -v /run/user/$(id -u):/tmp/runtime \
    -v ~/.gemini:/home/root/.gemini \
    -v $(pwd):/workspace \
    -e XDG_RUNTIME_DIR=/tmp/runtime \
    -e TERM=xterm-256color \
    -e COLORTERM=truecolor \
    taped-dev bash
```

## License

This project is licensed under the GPL - see the [LICENSE](LICENSE) file for details
