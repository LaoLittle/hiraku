# Hiraku Data Package v1

All integers use little-endian byte order. Paths are normalized UTF-8 paths
relative to the package root.

## Volume header

Every volume starts with the same 52-byte header. Its fixed-width wire layout
is read and written with `zerocopy` little-endian integer wrappers:

| Offset | Size | Value |
| ---: | ---: | --- |
| 0 | 4 | `HDP\0` |
| 4 | 2 | format version |
| 6 | 2 | header size |
| 8 | 4 | reserved flags |
| 12 | 8 | deterministic package ID |
| 20 | 4 | zero-based volume index |
| 24 | 4 | volume count |
| 28 | 8 | index size; zero outside volume 0 |
| 36 | 8 | first data offset |
| 44 | 8 | index checksum; zero outside volume 0 |

Volume 0 stores the complete file/chunk index immediately after its header.
Other volumes contain a header followed by chunk data. A single desktop HDP is
volume 0 with `volume_count = 1`; split packages use `main.hdp`,
`main.hdp.001`, `main.hdp.002`, and so on.

## File index

The index starts with a `u32` file count. Each file contains:

- `u32` path byte length and UTF-8 path bytes;
- `u64` decoded file size;
- `u32` chunk count;
- fixed-size chunk descriptors.

Each 40-byte chunk descriptor contains:

| Size | Value |
| ---: | --- |
| 4 | volume index |
| 1 | compression method ID |
| 1 | encryption method ID |
| 2 | reserved |
| 8 | absolute offset in the volume |
| 8 | stored size |
| 8 | decoded size |
| 8 | decoded-data checksum |

Compression method `0` is stored and `1` is zstd. Encryption method `0` is
none. Codec and encryption IDs are independent so future algorithms do not
change the index representation.

Files are split before compression. Every chunk is an independent compression
frame and can therefore be fetched, decoded, and verified without reading a
previous chunk. Compression occurs before any future encryption step.

Checksums and the deterministic package ID currently use specified FNV-1a
64-bit hashing for corruption detection. They are not cryptographic signatures.
