/**
 * Type guard to check if an object has a specific key.
 */
export function hasKey<K extends string>(obj: object, key: K): obj is { [P in K]: unknown } {
  return key in obj;
}

/**
 * Check if a value is structurally assignable to a schema type.
 *
 * @param value - The value to check
 * @param schema - The schema/default value to check against
 * @param nullSchemaAcceptsAny - If true, a null schema accepts any value.
 *                               If false, null schema requires null value.
 */
export function isAssignableTo<T>(value: unknown, schema: T, nullSchemaAcceptsAny: boolean): value is T {
  if (nullSchemaAcceptsAny && schema === null) return true;
  if (typeof schema !== typeof value) return false;
  if (typeof value === 'object' && typeof schema === 'object') {
    if (schema === null) return value === null;
    if (value === null) return false;
    if (Array.isArray(schema)) {
      if (!Array.isArray(value)) return false;
      if (schema.length === 0) return true;
      return value.every(item => isAssignableTo(item, schema[0], nullSchemaAcceptsAny));
    }
    for (const [key, schemaValue] of Object.entries(schema)) {
      if (!hasKey(value, key)) return false;
      if (!isAssignableTo(value[key], schemaValue, nullSchemaAcceptsAny)) return false;
    }
  }
  return true;
}
