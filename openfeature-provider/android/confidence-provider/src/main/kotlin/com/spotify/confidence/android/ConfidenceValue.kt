package com.spotify.confidence.android

import java.util.Date

/**
 * Sealed interface representing all possible Confidence value types.
 * Mirrors the existing Confidence SDK's value system for compatibility.
 */
sealed interface ConfidenceValue {
    data class String(val string: kotlin.String) : ConfidenceValue {
        override fun toString() = string
    }

    data class Double(val double: kotlin.Double) : ConfidenceValue {
        override fun toString() = double.toString()
    }

    data class Boolean(val boolean: kotlin.Boolean) : ConfidenceValue {
        override fun toString() = boolean.toString()
    }

    data class Integer(val integer: Int) : ConfidenceValue {
        override fun toString() = integer.toString()
    }

    data class Struct(val map: Map<kotlin.String, ConfidenceValue>) : ConfidenceValue {
        override fun toString() = map.toString()

        fun getValue(key: kotlin.String): ConfidenceValue? = map[key]
    }

    data class List(val list: kotlin.collections.List<ConfidenceValue>) : ConfidenceValue {
        override fun toString() = list.toString()
    }

    data class Timestamp(val dateTime: Date) : ConfidenceValue {
        override fun toString() = dateTime.toString()
    }

    object Null : ConfidenceValue {
        override fun toString() = "null"
    }

    companion object {
        fun stringList(list: kotlin.collections.List<kotlin.String>) =
            List(list.map(ConfidenceValue::String))

        fun doubleList(list: kotlin.collections.List<kotlin.Double>) =
            List(list.map(ConfidenceValue::Double))

        fun booleanList(list: kotlin.collections.List<kotlin.Boolean>) =
            List(list.map(ConfidenceValue::Boolean))

        fun integerList(list: kotlin.collections.List<kotlin.Int>) =
            List(list.map(ConfidenceValue::Integer))
    }
}

/**
 * Type alias for context maps.
 */
typealias ConfidenceValueMap = Map<kotlin.String, ConfidenceValue>
