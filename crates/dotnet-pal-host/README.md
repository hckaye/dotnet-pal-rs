# dotnet-pal-host

Validated foreign C host-table providers. Enable only required host capability groups and call `validate::<VM>()` from the composing port validation hook. Does not supply an OS implementation.

Part of the dotnet-pal-rs workspace. Core contracts live in `dotnet-pal-rs`;
platform implementations are separate dependencies. See the repository
`docs/crate-boundaries.md` for migration and supported qualification profiles.
