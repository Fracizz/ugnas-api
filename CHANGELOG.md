# Changelog

Release notes are written in English by default. See
[CHANGELOG.zh-CN.md](CHANGELOG.zh-CN.md) for the Chinese translation.

## 0.3.0

- Added resumable chunked uploads with server-confirmed offsets, retries,
  progress reporting, and cancellation.
- Made downloads resumable and atomic through validated `.part` files.
- Added structured API, authentication, network, timeout, and transfer
  exceptions.
- Rewind seekable request bodies after authentication renewal and reject unsafe
  request replays.
- Added explicit upload-to-directory and upload-to-path APIs.
- Added profile-isolated configuration and Windows DPAPI password storage.
- Redacted secrets embedded in CLI URLs and added safer password input options.
- Added pagination helpers, operation-timeout enforcement, CI, typing metadata,
  and broader tests.

## 0.2.2

- Integrated the UGOS two-stage upload protocol and package-level protocol
  upload tests.
