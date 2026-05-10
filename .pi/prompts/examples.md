---
description: Build local Docker images, run each state's README example against its container, and overwrite the GPX files in routes/
---
Refresh the checked-in example GPX files by exercising the local Docker images.

Scope:
- Build local images with `make docker`.
- For each state image (`overlandr:oregon`, `overlandr:idaho`, `overlandr:montana`):
  1. start the container locally
  2. wait until the server is ready on port 3000
  3. run the matching example request from `README.md`
  4. overwrite the corresponding file in `routes/`
  5. stop and remove the container before moving to the next image

Example-to-image mapping:
- `overlandr:oregon` → `routes/199_california-astoria_oregon.gpx`
- `overlandr:idaho` → `routes/91_preston_idaho-2_montana.gpx`
- `overlandr:montana` → `routes/2_montana-59_wyoming.gpx`

Requirements:
1. Use the exact example request bodies from `README.md`.
2. Overwrite the existing GPX files in `routes/`.
3. If a container fails to start or route generation fails, capture the relevant logs/error and stop there.
4. Avoid changing `README.md` unless the example commands are actually wrong.
5. After finishing, report:
   - which images were built
   - which route files were regenerated
   - any container/runtime issues encountered

Be careful to run only one container on port 3000 at a time.