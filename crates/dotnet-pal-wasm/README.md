# dotnet-pal-wasm

Single-owner wasm32 memory.grow storage for explicit composition with the PAL. Must not compete with another linear-memory allocator.

Part of the dotnet-pal-rs workspace. Core contracts live in `dotnet-pal-rs`;
platform implementations are separate dependencies. See the repository
`docs/crate-boundaries.md` for migration and supported qualification profiles.
