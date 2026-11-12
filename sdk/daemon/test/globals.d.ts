// Type checking only. examples/link-channel/http-fixture.ts is copied
// unchanged from PocketJS, whose type environment has a global `BodyInit`;
// this package's (`lib: ["ESNext"]`, `types: ["bun"]`) has it as
// `import("bun").BodyInit`. Bun itself needs nothing at run time.
type BodyInit = import("bun").BodyInit;
