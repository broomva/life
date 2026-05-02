/**
 * `AnimaError` — typed errors for the browser custody surface.
 *
 * Mirrors Rust's `AnimaError` shape with a string `code` discriminator.
 * Browser code paths surface these as causes of `LifeSdkError` when
 * possible; standalone `AnimaError`s are thrown by the
 * `passkey` / `did` / `web_crypto` modules where there's no SDK
 * transport involved.
 */

import { LifeSdkError } from "../errors.js";

export class AnimaError extends LifeSdkError {
  constructor(code: string, message: string, options?: ErrorOptions) {
    super(code, message, options);
    this.name = "AnimaError";
    Object.setPrototypeOf(this, AnimaError.prototype);
  }

  /** Convenience — the underlying passkey API rejected the operation. */
  static passkey(message: string, cause?: unknown): AnimaError {
    return new AnimaError("passkey_failure", message, cause ? { cause } : undefined);
  }

  /** Convenience — remote `/anima/custody/*` HTTP route returned an error. */
  static remote(status: number, message: string): AnimaError {
    return new AnimaError(`remote_anima_${status}`, message);
  }

  /** Convenience — operation isn't supported on this backend. */
  static notSupported(message: string): AnimaError {
    return new AnimaError("not_supported", message);
  }

  /** Convenience — local state precondition failed (e.g. not enrolled). */
  static state(message: string): AnimaError {
    return new AnimaError("invalid_state", message);
  }

  /** Convenience — a cryptographic primitive failed (parsing, sig, etc.). */
  static crypto(message: string, cause?: unknown): AnimaError {
    return new AnimaError("crypto", message, cause ? { cause } : undefined);
  }
}
