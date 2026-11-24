# solidcraft-ipt-importer

An importer plugin for [SolidCraft](https://github.com/tangency-labs/solidcraft) that opens Autodesk
Inventor parts (`.ipt`). With it installed, File › Open, `doc.open` and `file.import` (CLI and MCP)
open `.ipt` files as new designs.

## How it works

SolidCraft reads FreeCAD and Rhino files through [cadmpeg](https://github.com/cadmpeg/cadmpeg). Its
build leaves out cadmpeg's codecs for commercial formats on purpose. This plugin compiles cadmpeg's
**Inventor codec** to WebAssembly and runs it inside SolidCraft's plugin sandbox:

1. It reads the `.ipt` through the importer `source` interface.
2. cadmpeg decodes the part's exact B-rep, stored in the file as ACIS, and writes it as STEP AP214.
   This is the same route SolidCraft takes for FreeCAD and Rhino files
   (`crates/io/src/native_cad/convert/step_route.rs`).
3. The plugin returns a design with one `import` feature that carries the STEP text inline.
   SolidCraft's own STEP import builds the solid when the design opens.
4. cadmpeg's losses, grouped by kind, become the import report. Most of them are Inventor feature
   history, which is not carried over.

The plugin asks for no permissions: no files, network, clock or input folder. It runs within
SolidCraft's default plugin limits (128 MiB of memory, 1e11 fuel).

## Status

Built against SolidCraft 0.48.1 (commit `e5bc9288`) and cadmpeg 0.6.12.

- **Parts tested:** several Inventor parts, with 35 to 385 faces each, imported as closed solids.
  Their volumes matched those from an independent conversion of the same files.
- **Speed:** a part takes about 0.3–3 s.
- **Known gap:** a part with a face built on an Inventor procedural surface that cadmpeg 0.6.12
  cannot decode imports as an open body.

**Limits:**
- Parts (`.ipt`) only, not assemblies (`.iam`).
- Shape only, no feature history.
- Colours are reduced to base colours.

## Build

You need Rust with the `wasm32-unknown-unknown` target and
[wasm-tools](https://github.com/bytecodealliance/wasm-tools):

```powershell
rustup target add wasm32-unknown-unknown
cargo install wasm-tools --locked
./build.ps1          # -> plugin/ipt-importer.wasm and dist/ipt-importer-<version>.zip
./build.ps1 -Check   # also runs `solidcraft-cli plugin check` (and `plugin test`, see below)
```

Two build details matter:

- **No wasm-bindgen.** SolidCraft's plugin host links neither WASI nor JavaScript. `zip`, which
  cadmpeg's container reader uses, enables getrandom's `wasm_js` backend on every wasm32 build, and
  that would import wasm-bindgen glue the host refuses. `vendor/zip` is zip 9.8.2 with that feature
  removed. `.cargo/config.toml` selects getrandom's custom backend instead, and `src/lib.rs`
  provides it. zip only needs randomness to write encrypted archives, which an importer never does.
- **Matching interface.** `wit/plugin.wit` is SolidCraft's plugin interface (`craft:plugin@0.2.0`)
  at commit `e5bc9288`. Rebuild against the WIT file of the SolidCraft version you run.

## Install

```powershell
$cli = "$env:ProgramFiles\SolidCraft\solidcraft-cli.exe"
& $cli exec plugin.install '{"path":"dist/ipt-importer-0.1.0.zip","dry_run":true}'   # prints approval + archive hashes
& $cli --keep-plugin-approvals exec plugin.install '{"path":"dist/ipt-importer-0.1.0.zip","approval":"<approval>","archive":"<archive>"}'
```

Add `"replace": true` to update an installed version. To remove the plugin, run
`& $cli exec plugin.remove '{"id":"ipt-importer"}'`. The MCP `plugin.install` cannot write to the
app's plugins folder, so install with the CLI.

## Tests

The repository ships no test cases or `.ipt` files; `plugin/tests/` is ignored. To test with your
own parts, put each `.ipt` in `plugin/tests/` beside a case file for `solidcraft-cli plugin test`.
The case below checks the body count and the imported volume:

```json
{
  "entry": "convert",
  "file": "part.ipt",
  "expect": {"reply": {"bodies": 1}, "model": {"total": {"volume_mm3": 1000.0}}}
}
```

`./build.ps1 -Check` runs `plugin test` when `plugin/tests/` holds `.ipt` files.

## Licence

Apache-2.0. See [NOTICE](NOTICE) for cadmpeg, the vendored zip (MIT) and SolidCraft's WIT file.
