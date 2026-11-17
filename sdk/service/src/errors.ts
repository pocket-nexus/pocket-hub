// Errors and JSON values as the reference service API names them
// (vendor/pocketjs/tools/pocket-link/v2/peer.ts `LinkError`, `Json`), with `ServiceError` for its `LinkError`.

export type Json = Record<string, unknown>;

/**
 * An error reply (link specification §5.2), or an outcome this side reports
 * for a call of its own: `closed` when the channel ended under it,
 * `no_channel` for a call made on a closed channel, `busy` and `too_large`
 * for a call refused before it reached the wire, `transfer` for a stream
 * that was reset. `extra` holds the error object's other fields.
 */
export class ServiceError extends Error {
  constructor(
    readonly code: string,
    message: string = code,
    readonly extra: Json = {},
  ) {
    super(message);
    this.name = "ServiceError";
  }
}

export const asServiceError = (error: unknown): ServiceError =>
  error instanceof ServiceError ? error : new ServiceError("internal", error instanceof Error ? error.message : String(error));
