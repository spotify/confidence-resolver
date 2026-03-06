@file:OptIn(kotlin.time.ExperimentalTime::class)

package com.spotify.confidence.android

import dev.openfeature.kotlin.sdk.EvaluationContext
import dev.openfeature.kotlin.sdk.Value
import java.util.Date

/**
 * Extension functions for converting between OpenFeature Value and ConfidenceValue types.
 */

/**
 * Converts an OpenFeature Value to a ConfidenceValue.
 */
fun Value.toConfidenceValue(): ConfidenceValue = when (this) {
    is Value.Null -> ConfidenceValue.Null
    is Value.Boolean -> ConfidenceValue.Boolean(this.boolean)
    is Value.Integer -> ConfidenceValue.Integer(this.integer)
    is Value.Double -> ConfidenceValue.Double(this.double)
    is Value.String -> ConfidenceValue.String(this.string)
    is Value.List -> ConfidenceValue.List(this.list.map { it.toConfidenceValue() })
    is Value.Structure -> ConfidenceValue.Struct(this.structure.mapValues { it.value.toConfidenceValue() })
    is Value.Instant -> ConfidenceValue.Timestamp(Date(this.instant.epochSeconds * 1000))
}

/**
 * Converts a ConfidenceValue to an OpenFeature Value.
 */
fun ConfidenceValue.toValue(): Value = when (this) {
    is ConfidenceValue.Boolean -> Value.Boolean(this.boolean)
    is ConfidenceValue.Double -> Value.Double(this.double)
    is ConfidenceValue.Integer -> Value.Integer(this.integer)
    is ConfidenceValue.List -> Value.List(this.list.map { it.toValue() })
    ConfidenceValue.Null -> Value.Null
    is ConfidenceValue.String -> Value.String(this.string)
    is ConfidenceValue.Struct -> Value.Structure(this.map.mapValues { it.value.toValue() })
    is ConfidenceValue.Timestamp -> Value.Instant(
        kotlin.time.Instant.fromEpochMilliseconds(this.dateTime.time)
    )
}

/**
 * Converts an EvaluationContext to a ConfidenceValue.Struct.
 */
fun EvaluationContext.toConfidenceContext(): ConfidenceValue.Struct {
    val map = mutableMapOf<String, ConfidenceValue>()

    // Add targeting key
    map["targeting_key"] = ConfidenceValue.String(getTargetingKey())

    // Add all attributes
    asMap().forEach { (key, value) ->
        map[key] = value.toConfidenceValue()
    }

    return ConfidenceValue.Struct(map)
}

/**
 * Extracts a value at the given path from a ConfidenceValue.Struct.
 */
fun findValueFromPath(value: ConfidenceValue.Struct, path: List<String>): ConfidenceValue? {
    if (path.isEmpty()) return value

    val currValue = value.map[path[0]] ?: return null

    return when {
        currValue is ConfidenceValue.Struct && path.size > 1 -> {
            findValueFromPath(currValue, path.subList(1, path.size))
        }
        path.size == 1 -> currValue
        else -> null
    }
}

/**
 * Converts a ResolveReason to an OpenFeature reason string.
 */
fun ResolveReason.toOpenFeatureReason(): String = when (this) {
    ResolveReason.RESOLVE_REASON_MATCH -> "TARGETING_MATCH"
    ResolveReason.RESOLVE_REASON_STALE -> "STALE"
    ResolveReason.ERROR -> "ERROR"
    ResolveReason.RESOLVE_REASON_TARGETING_KEY_ERROR -> "ERROR"
    ResolveReason.RESOLVE_REASON_UNSPECIFIED -> "UNKNOWN"
    else -> "DEFAULT"
}

/**
 * Converts an ErrorCode to an OpenFeature error code.
 */
fun ErrorCode.toOpenFeatureErrorCode(): dev.openfeature.kotlin.sdk.exceptions.ErrorCode = when (this) {
    ErrorCode.FLAG_NOT_FOUND -> dev.openfeature.kotlin.sdk.exceptions.ErrorCode.FLAG_NOT_FOUND
    ErrorCode.INVALID_CONTEXT -> dev.openfeature.kotlin.sdk.exceptions.ErrorCode.INVALID_CONTEXT
    ErrorCode.PARSE_ERROR -> dev.openfeature.kotlin.sdk.exceptions.ErrorCode.PARSE_ERROR
    ErrorCode.PROVIDER_NOT_READY -> dev.openfeature.kotlin.sdk.exceptions.ErrorCode.PROVIDER_NOT_READY
    ErrorCode.GENERAL -> dev.openfeature.kotlin.sdk.exceptions.ErrorCode.GENERAL
}
