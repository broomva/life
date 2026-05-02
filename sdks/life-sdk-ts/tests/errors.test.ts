/**
 * Close-code → typed error mapping (Spec C₃ §6.5).
 *
 * The mapping is canonicalised in
 * `docs/superpowers/specs/2026-04-29-spec-c3-close-codes.md`.
 */

import { describe, expect, it } from "vitest";
import {
  AuthError,
  BackpressureError,
  GoingAwayError,
  GrpcCode,
  InternalServerError,
  IpBlockedError,
  LifedUnavailableError,
  LifeSdkError,
  RateLimitError,
  SequenceRetiredError,
  closeCodeToError,
  grpcCodeToError,
} from "../src/errors.js";

describe("closeCodeToError", () => {
  it("returns null for graceful 1000", () => {
    expect(closeCodeToError(1000, "normal")).toBeNull();
  });

  it("maps 1001 to GoingAwayError", () => {
    const err = closeCodeToError(1001, "going_away");
    expect(err).toBeInstanceOf(GoingAwayError);
    expect(err?.code).toBe("going_away");
  });

  it("maps 1008 to AuthError (token expired)", () => {
    const err = closeCodeToError(1008, "policy_violation:token_expired");
    expect(err).toBeInstanceOf(AuthError);
    expect(err?.code).toBe("unauthenticated");
  });

  it("maps 1011 to InternalServerError", () => {
    const err = closeCodeToError(1011, "internal_error");
    expect(err).toBeInstanceOf(InternalServerError);
  });

  it("maps 4001 to RateLimitError with reason prefix", () => {
    const err = closeCodeToError(4001, "rate_limit:per_user") as RateLimitError;
    expect(err).toBeInstanceOf(RateLimitError);
    expect(err.reasonPrefix).toBe("rate_limit:per_user");
  });

  it("preserves per_ip variant in RateLimitError", () => {
    const err = closeCodeToError(4001, "rate_limit:per_ip") as RateLimitError;
    expect(err.reasonPrefix).toBe("rate_limit:per_ip");
  });

  it("maps 4002 to BackpressureError", () => {
    expect(closeCodeToError(4002, "backpressure:slow_consumer")).toBeInstanceOf(
      BackpressureError,
    );
  });

  it("maps 4003 to IpBlockedError", () => {
    expect(closeCodeToError(4003, "ip_blocked")).toBeInstanceOf(IpBlockedError);
  });

  it("maps 4004 to LifedUnavailableError", () => {
    expect(closeCodeToError(4004, "lifed_circuit_open")).toBeInstanceOf(
      LifedUnavailableError,
    );
  });

  it("maps 4005 to SequenceRetiredError", () => {
    expect(closeCodeToError(4005, "sequence_retired")).toBeInstanceOf(
      SequenceRetiredError,
    );
  });

  it("returns null for unmapped codes (e.g. 1006)", () => {
    expect(closeCodeToError(1006)).toBeNull();
  });
});

describe("grpcCodeToError", () => {
  it("maps UNAUTHENTICATED to AuthError", () => {
    expect(grpcCodeToError(GrpcCode.Unauthenticated, "UNAUTHENTICATED")).toBeInstanceOf(
      AuthError,
    );
  });

  it("maps PERMISSION_DENIED to AuthError", () => {
    expect(grpcCodeToError(GrpcCode.PermissionDenied, "PERMISSION_DENIED")).toBeInstanceOf(
      AuthError,
    );
  });

  it("maps RESOURCE_EXHAUSTED to RateLimitError", () => {
    expect(grpcCodeToError(GrpcCode.ResourceExhausted, "RESOURCE_EXHAUSTED")).toBeInstanceOf(
      RateLimitError,
    );
  });

  it("maps UNAVAILABLE to LifedUnavailableError", () => {
    expect(grpcCodeToError(GrpcCode.Unavailable, "UNAVAILABLE")).toBeInstanceOf(
      LifedUnavailableError,
    );
  });

  it("maps OUT_OF_RANGE to SequenceRetiredError", () => {
    expect(grpcCodeToError(GrpcCode.OutOfRange, "OUT_OF_RANGE")).toBeInstanceOf(
      SequenceRetiredError,
    );
  });

  it("maps INTERNAL to InternalServerError", () => {
    expect(grpcCodeToError(GrpcCode.Internal, "INTERNAL")).toBeInstanceOf(
      InternalServerError,
    );
  });

  it("returns LifeSdkError for any code (parent type)", () => {
    expect(grpcCodeToError(GrpcCode.Cancelled, "CANCELLED")).toBeInstanceOf(
      LifeSdkError,
    );
  });
});
