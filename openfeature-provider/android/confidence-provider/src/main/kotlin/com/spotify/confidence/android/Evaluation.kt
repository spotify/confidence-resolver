package com.spotify.confidence.android

/**
 * Represents the result of a flag evaluation.
 */
data class Evaluation<T>(
    val value: T,
    val variant: String? = null,
    val reason: ResolveReason,
    val errorCode: ErrorCode? = null,
    val errorMessage: String? = null
)

/**
 * Enum representing the reason for a flag resolution result.
 */
enum class ResolveReason {
    /** Unspecified enum. */
    RESOLVE_REASON_UNSPECIFIED,

    /** The flag was successfully resolved because one rule matched. */
    RESOLVE_REASON_MATCH,

    /** The flag value is from cache and may be stale. */
    RESOLVE_REASON_STALE,

    /** The flag could not be resolved because no rule matched. */
    RESOLVE_REASON_NO_SEGMENT_MATCH,

    /** The flag could not be resolved because the matching rule had no variant. */
    RESOLVE_REASON_NO_TREATMENT_MATCH,

    /** The flag could not be resolved because the targeting key is invalid. */
    RESOLVE_REASON_TARGETING_KEY_ERROR,

    /** The flag could not be resolved because it was archived. */
    RESOLVE_REASON_FLAG_ARCHIVED,

    /** Default fallback reason. */
    DEFAULT,

    /** An error occurred during evaluation. */
    ERROR
}

/**
 * Error codes for flag evaluation errors.
 */
enum class ErrorCode {
    /** The provider is not ready yet. */
    PROVIDER_NOT_READY,

    /** The requested flag was not found. */
    FLAG_NOT_FOUND,

    /** Error parsing the flag value. */
    PARSE_ERROR,

    /** The evaluation context is invalid. */
    INVALID_CONTEXT,

    /** General error. */
    GENERAL
}
