// Stands for the SDK's bootstrap in the supervisor's tests: the host runs
// `bun --no-install run <sdk>/src/boot.ts ./<entry>` (SERVICE-PROTOCOL.md §2),
// and this one runs the entry as a program, so a fixture speaks the protocol
// at the frame level and can break it.
import { resolve } from "node:path";

await import(resolve(process.argv[2]!));
