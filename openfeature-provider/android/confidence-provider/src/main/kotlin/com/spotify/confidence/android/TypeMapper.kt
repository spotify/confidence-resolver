@file:OptIn(kotlin.time.ExperimentalTime::class)

package com.spotify.confidence.android

import com.google.protobuf.ListValue
import com.google.protobuf.NullValue
import com.google.protobuf.Struct
import com.spotify.confidence.flags.types.v1.FlagSchema
import dev.openfeature.kotlin.sdk.EvaluationContext
import dev.openfeature.kotlin.sdk.Value

/**
 * Utility object for converting between OpenFeature types and Protobuf types.
 */
internal object TypeMapper {

    /**
     * Converts a protobuf Value with its schema to an OpenFeature Value.
     */
    fun fromProto(value: com.google.protobuf.Value, schema: FlagSchema): Value {
        if (schema.schemaTypeCase == FlagSchema.SchemaTypeCase.SCHEMATYPE_NOT_SET) {
            throw IllegalArgumentException("schemaType not set in FlagSchema")
        }

        val mismatchPrefix = "Mismatch between schema and value:"

        return when (value.kindCase) {
            com.google.protobuf.Value.KindCase.NULL_VALUE -> Value.Null

            com.google.protobuf.Value.KindCase.NUMBER_VALUE -> {
                when (schema.schemaTypeCase) {
                    FlagSchema.SchemaTypeCase.INT_SCHEMA -> {
                        val intVal = value.numberValue.toInt()
                        if (intVal.toDouble() != value.numberValue) {
                            throw IllegalArgumentException("$mismatchPrefix value should be an int, but it is a double/long")
                        }
                        Value.Integer(intVal)
                    }
                    FlagSchema.SchemaTypeCase.DOUBLE_SCHEMA -> Value.Double(value.numberValue)
                    else -> throw IllegalArgumentException("Number field must have schema type int or double")
                }
            }

            com.google.protobuf.Value.KindCase.STRING_VALUE -> {
                if (schema.schemaTypeCase != FlagSchema.SchemaTypeCase.STRING_SCHEMA) {
                    throw IllegalArgumentException("$mismatchPrefix value is a String, but it should be something else")
                }
                Value.String(value.stringValue)
            }

            com.google.protobuf.Value.KindCase.BOOL_VALUE -> {
                if (schema.schemaTypeCase != FlagSchema.SchemaTypeCase.BOOL_SCHEMA) {
                    throw IllegalArgumentException("$mismatchPrefix value is a bool, but should be something else")
                }
                Value.Boolean(value.boolValue)
            }

            com.google.protobuf.Value.KindCase.STRUCT_VALUE -> {
                if (schema.schemaTypeCase != FlagSchema.SchemaTypeCase.STRUCT_SCHEMA) {
                    throw IllegalArgumentException("$mismatchPrefix value is a struct, but should be something else")
                }
                fromProto(value.structValue, schema.structSchema)
            }

            com.google.protobuf.Value.KindCase.LIST_VALUE -> {
                if (schema.schemaTypeCase != FlagSchema.SchemaTypeCase.LIST_SCHEMA) {
                    throw IllegalArgumentException("$mismatchPrefix value is a list, but should be something else")
                }
                val mappedList = value.listValue.valuesList.map { v ->
                    fromProto(v, schema.listSchema.elementSchema)
                }
                Value.List(mappedList)
            }

            com.google.protobuf.Value.KindCase.KIND_NOT_SET ->
                throw IllegalArgumentException("kind not set in com.google.protobuf.Value")

            else -> throw IllegalArgumentException("Unknown value type")
        }
    }

    /**
     * Converts a protobuf Struct with its schema to an OpenFeature Value.
     */
    fun fromProto(struct: Struct, schema: FlagSchema.StructFlagSchema): Value {
        val map = struct.fieldsMap.entries.associate { (key, value) ->
            if (!schema.schemaMap.containsKey(key)) {
                throw IllegalArgumentException("Lacking schema for field '$key'")
            }
            key to fromProto(value, schema.schemaMap[key]!!)
        }
        return Value.Structure(map)
    }

    /**
     * Converts an OpenFeature Value to a protobuf Value.
     */
    fun toProto(value: Value): com.google.protobuf.Value {
        return when (value) {
            is Value.Null -> com.google.protobuf.Value.newBuilder()
                .setNullValue(NullValue.NULL_VALUE)
                .build()

            is Value.Boolean -> com.google.protobuf.Value.newBuilder()
                .setBoolValue(value.boolean)
                .build()

            is Value.Integer -> com.google.protobuf.Value.newBuilder()
                .setNumberValue(value.integer.toDouble())
                .build()

            is Value.Double -> com.google.protobuf.Value.newBuilder()
                .setNumberValue(value.double)
                .build()

            is Value.String -> com.google.protobuf.Value.newBuilder()
                .setStringValue(value.string)
                .build()

            is Value.List -> com.google.protobuf.Value.newBuilder()
                .setListValue(
                    ListValue.newBuilder()
                        .addAllValues(value.list.map { toProto(it) })
                        .build()
                )
                .build()

            is Value.Structure -> {
                val protoMap = value.structure.entries.associate { (key, v) ->
                    key to toProto(v)
                }
                com.google.protobuf.Value.newBuilder()
                    .setStructValue(Struct.newBuilder().putAllFields(protoMap).build())
                    .build()
            }

            is Value.Instant -> com.google.protobuf.Value.newBuilder()
                .setStringValue(value.instant.toString())
                .build()
        }
    }

    /**
     * Converts an OpenFeature EvaluationContext to a protobuf Struct.
     */
    fun evaluationContextToStruct(context: EvaluationContext?): Struct {
        if (context == null) {
            return Struct.getDefaultInstance()
        }

        val builder = Struct.newBuilder()

        // Add targeting key
        builder.putFields(
            "targeting_key",
            com.google.protobuf.Value.newBuilder()
                .setStringValue(context.getTargetingKey())
                .build()
        )

        // Add all context attributes
        context.asMap().forEach { (key, value) ->
            builder.putFields(key, toProto(value))
        }

        return builder.build()
    }
}
