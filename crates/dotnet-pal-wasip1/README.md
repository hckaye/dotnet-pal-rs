# dotnet-pal-wasip1

WASI Preview 1 clock, environment, entropy, identity and dispatch providers. Compose explicit storage separately. Only the wasm32-wasip1 target is implemented.

Part of the dotnet-pal-rs workspace. Core contracts live in `dotnet-pal-rs`;
platform implementations are separate dependencies. See the repository
`docs/crate-boundaries.md` for migration and supported qualification profiles.
