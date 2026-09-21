# Flatpak packaging

`io.github.MoisesRoig.cosmic-ext-applet-agents.json` is the manifest submitted to
[pop-os/cosmic-flatpak](https://github.com/pop-os/cosmic-flatpak), the repository
the COSMIC Store reads for applets.

It is built alongside a `cargo-sources.json` that vendors every crate, which is
regenerated from `Cargo.lock` on each release and lives only in the submission:

```sh
curl -sSLO https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
uv run flatpak-cargo-generator.py Cargo.lock -o cargo-sources.json
```

On every release, bump `tag` and `commit` in the manifest, regenerate
`cargo-sources.json`, and open a pull request against that repository.
