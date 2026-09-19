# dotnet-pal-std

Compatibility facade selecting only the target OS provider. New platform-specific consumers can depend on dotnet-pal-linux-std, dotnet-pal-macos or dotnet-pal-windows directly.

Part of the dotnet-pal-rs workspace. Core contracts live in `dotnet-pal-rs`;
platform implementations are separate dependencies. See the repository
`docs/crate-boundaries.md` for migration and supported qualification profiles.
