package confidence

import (
	"errors"
	"fmt"
	"math"
	"reflect"
	"strings"

	"google.golang.org/protobuf/types/known/structpb"
)

// unmarshalProto converts a protobuf Value to a typed Go value matching defaultValue.
// Returns an error if proto value isn't assignable to the type of defaultValue.
// For maps, the defaultValue is copied first and then proto values are overlaid.
func unmarshalProto[T any](protoValue *structpb.Value, defaultValue T, path []string) (T, error) {
	if protoValue == nil {
		// TODO should this be error or not?
		return defaultValue, nil
	}

	rv := reflect.ValueOf(defaultValue)

	// Handle nil defaultValue (e.g., any(nil)) - use AsInterface() for dynamic typing
	if !rv.IsValid() {
		result := protoValue.AsInterface()
		if result == nil {
			return defaultValue, nil
		}
		return result.(T), nil
	}

	rt := rv.Type()

	isPtr := rt.Kind() == reflect.Pointer
	if isPtr {
		rt = rt.Elem()
		if !rv.IsNil() {
			rv = rv.Elem()
		} else {
			rv = reflect.Value{} // No default value to copy
		}
	}

	result, err := unmarshalProtoValue(protoValue, rt, rv, path)
	if err != nil {
		return defaultValue, err
	}

	// Convert result to T
	if isPtr {
		ptr := reflect.New(rt)
		ptr.Elem().Set(result)
		return ptr.Interface().(T), nil
	}
	return result.Interface().(T), nil
}

// unmarshalProtoValue recursively converts a protobuf Value to a reflect.Value of the target type.
// defaultValue is used to copy existing values for maps (can be invalid/zero if no default).
func unmarshalProtoValue(protoValue *structpb.Value, targetType reflect.Type, defaultValue reflect.Value, path []string) (reflect.Value, error) {
	// Handle nil proto pointer - return default or zero value
	if protoValue == nil {
		if defaultValue.IsValid() {
			return defaultValue, nil
		}
		return reflect.Zero(targetType), nil
	}

	// Handle null proto value - return default or zero value
	if _, ok := protoValue.Kind.(*structpb.Value_NullValue); ok {
		if defaultValue.IsValid() {
			return defaultValue, nil
		}
		return reflect.Zero(targetType), nil
	}

	switch targetType.Kind() {
	case reflect.Struct:
		return unmarshalStruct(protoValue, targetType, defaultValue, path)

	case reflect.Map:
		return unmarshalMap(protoValue, targetType, defaultValue, path)

	case reflect.Slice:
		return reflect.Value{}, formatError("slice types are not supported", path)

	case reflect.String:
		if s, ok := protoValue.Kind.(*structpb.Value_StringValue); ok {
			return reflect.ValueOf(s.StringValue), nil
		}
		return reflect.Value{}, formatMismatchError("string", protoValue.Kind, path)

	case reflect.Bool:
		if b, ok := protoValue.Kind.(*structpb.Value_BoolValue); ok {
			return reflect.ValueOf(b.BoolValue), nil
		}
		return reflect.Value{}, formatMismatchError("bool", protoValue.Kind, path)

	case reflect.Float64:
		if n, ok := protoValue.Kind.(*structpb.Value_NumberValue); ok {
			return reflect.ValueOf(n.NumberValue), nil
		}
		return reflect.Value{}, formatMismatchError("float64", protoValue.Kind, path)

	case reflect.Float32:
		if n, ok := protoValue.Kind.(*structpb.Value_NumberValue); ok {
			return reflect.ValueOf(float32(n.NumberValue)), nil
		}
		return reflect.Value{}, formatMismatchError("float32", protoValue.Kind, path)

	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		if n, ok := protoValue.Kind.(*structpb.Value_NumberValue); ok {
			if n.NumberValue != math.Trunc(n.NumberValue) {
				return reflect.Value{}, formatError(
					fmt.Sprintf("resolved value (%v) is not a whole number, cannot convert to %v", n.NumberValue, targetType),
					path,
				)
			}
			return reflect.ValueOf(n.NumberValue).Convert(targetType), nil
		}
		return reflect.Value{}, formatMismatchError(targetType.String(), protoValue.Kind, path)

	case reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
		if n, ok := protoValue.Kind.(*structpb.Value_NumberValue); ok {
			if n.NumberValue < 0 {
				return reflect.Value{}, formatError(
					fmt.Sprintf("resolved value (%v) is negative, cannot convert to %v", n.NumberValue, targetType),
					path,
				)
			}
			if n.NumberValue != math.Trunc(n.NumberValue) {
				return reflect.Value{}, formatError(
					fmt.Sprintf("resolved value (%v) is not a whole number, cannot convert to %v", n.NumberValue, targetType),
					path,
				)
			}
			return reflect.ValueOf(n.NumberValue).Convert(targetType), nil
		}
		return reflect.Value{}, formatMismatchError(targetType.String(), protoValue.Kind, path)

	case reflect.Interface:
		// If we have a default value, use its type to guide validation
		if defaultValue.IsValid() && !defaultValue.IsNil() {
			actualType := defaultValue.Elem().Type()
			return unmarshalProtoValue(protoValue, actualType, defaultValue.Elem(), path)
		}
		// For interface{}/any with no default, return the natural Go representation
		return reflect.ValueOf(protoValue.AsInterface()), nil

	case reflect.Pointer:
		// Handle pointer to other types
		elemType := targetType.Elem()
		var elemDefault reflect.Value
		if defaultValue.IsValid() && !defaultValue.IsNil() {
			elemDefault = defaultValue.Elem()
		}
		elemValue, err := unmarshalProtoValue(protoValue, elemType, elemDefault, path)
		if err != nil {
			return reflect.Value{}, err
		}
		ptr := reflect.New(elemType)
		ptr.Elem().Set(elemValue)
		return ptr, nil

	default:
		return reflect.Value{}, formatMismatchError(targetType.String(), protoValue.Kind, path)
	}
}

// unmarshalStruct converts a protobuf Struct to a Go struct.
// All exported struct fields must be present in the proto, otherwise an error is returned.
// If defaultValue is valid, its field values are used when proto fields are null.
func unmarshalStruct(protoValue *structpb.Value, targetType reflect.Type, defaultValue reflect.Value, path []string) (reflect.Value, error) {
	protoStruct, ok := protoValue.Kind.(*structpb.Value_StructValue)
	if !ok || protoStruct.StructValue == nil {
		return reflect.Value{}, formatMismatchError("struct", protoValue.Kind, path)
	}

	result := reflect.New(targetType).Elem()
	fields := protoStruct.StructValue.Fields

	for i := 0; i < targetType.NumField(); i++ {
		field := targetType.Field(i)

		// Skip unexported fields
		if !field.IsExported() {
			continue
		}

		name := getFieldName(field)
		path := append(path, name)
		pbFieldValue, exists := fields[name]
		if !exists {
			return reflect.Value{}, fmt.Errorf("resolved value is missing field %v", strings.Join(path, "."))
		}

		// Extract field default from defaultValue if available
		var fieldDefault reflect.Value
		if defaultValue.IsValid() {
			fieldDefault = defaultValue.Field(i)
		}

		fieldValue, err := unmarshalProtoValue(pbFieldValue, field.Type, fieldDefault, path)
		if err != nil {
			return reflect.Value{}, err
		}

		result.Field(i).Set(fieldValue)
	}

	return result, nil
}

// unmarshalMap converts a protobuf Struct to a Go map.
// If defaultValue is a valid map, its entries are copied first, then proto values are overlaid.
func unmarshalMap(protoValue *structpb.Value, targetType reflect.Type, defaultValue reflect.Value, path []string) (reflect.Value, error) {
	protoStruct, ok := protoValue.Kind.(*structpb.Value_StructValue)
	if !ok || protoStruct.StructValue == nil {
		return reflect.Value{}, formatMismatchError("map", protoValue.Kind, path)
	}

	if targetType.Key().Kind() != reflect.String {
		return reflect.Value{}, formatError("map keys must be strings", path)
	}

	result := reflect.MakeMap(targetType)
	elemType := targetType.Elem()

	// Copy existing entries from default value first
	if defaultValue.IsValid() && !defaultValue.IsNil() && defaultValue.Type() == targetType {
		for _, key := range defaultValue.MapKeys() {
			result.SetMapIndex(key, defaultValue.MapIndex(key))
		}
	}

	// Overlay with proto values
	for key, pbValue := range protoStruct.StructValue.Fields {
		// Get existing value as default for nested unmarshaling
		keyVal := reflect.ValueOf(key)
		var elemDefault reflect.Value
		if defaultValue.IsValid() && !defaultValue.IsNil() {
			elemDefault = defaultValue.MapIndex(keyVal)
		}

		elemValue, err := unmarshalProtoValue(pbValue, elemType, elemDefault, append(path, key))
		if err != nil {
			return reflect.Value{}, err
		}
		result.SetMapIndex(keyVal, elemValue)
	}

	return result, nil
}

// getFieldName returns the name to use for matching against protobuf fields.
// Uses json tag if present, otherwise the field name in snake_case.
func getFieldName(field reflect.StructField) string {
	// Check for json tag first
	if tag := field.Tag.Get("json"); tag != "" {
		parts := strings.Split(tag, ",")
		if parts[0] != "" && parts[0] != "-" {
			return parts[0]
		}
	}

	// Fall back to snake_case conversion of field name
	return toSnakeCase(field.Name)
}

// toSnakeCase converts CamelCase to snake_case, handling acronyms correctly.
// e.g., "HTTPServer" -> "http_server", "userID" -> "user_id"
func toSnakeCase(s string) string {
	if len(s) == 0 {
		return s
	}

	runes := []rune(s)
	var result strings.Builder

	for i, r := range runes {
		if i > 0 && r >= 'A' && r <= 'Z' {
			prev := runes[i-1]
			prevIsLower := prev >= 'a' && prev <= 'z'
			prevIsUpper := prev >= 'A' && prev <= 'Z'
			prevIsDigit := prev >= '0' && prev <= '9'

			// Insert underscore if previous char was lowercase or digit
			if prevIsLower || prevIsDigit {
				result.WriteByte('_')
			} else if prevIsUpper {
				// Previous was also uppercase (in an acronym)
				// Insert underscore if next char is lowercase (end of acronym)
				if i+1 < len(runes) && runes[i+1] >= 'a' && runes[i+1] <= 'z' {
					result.WriteByte('_')
				}
			}
		}
		result.WriteRune(r)
	}
	return strings.ToLower(result.String())
}

// pathString formats a path slice as a dotted string for error messages.
func formatError(msg string, path []string) error {
	if len(path) > 0 {
		msg = fmt.Sprintf("%s, at %s", msg, strings.Join(path, "."))
	}
	return errors.New(msg)
}

func formatMismatchError(defaultType string, protoKind any, path []string) error {
	return formatError(fmt.Sprintf("resolved value (%v) not assignable to default type (%v)", protoKindName(protoKind), defaultType), path)
}

func protoKindName(kind any) string {
	switch kind.(type) {
	case *structpb.Value_StringValue:
		return "string"
	case *structpb.Value_NumberValue:
		return "number"
	case *structpb.Value_BoolValue:
		return "bool"
	case *structpb.Value_StructValue:
		return "struct"
	case *structpb.Value_ListValue:
		return "list"
	case *structpb.Value_NullValue:
		return "null"
	default:
		return fmt.Sprintf("%T", kind)
	}
}
