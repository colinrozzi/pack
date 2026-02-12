# Interface Hashing: Merkle Trees for Type Compatibility

## Overview

Pack uses **Merkle-tree hashing** to enable O(1) interface compatibility checking. Every type, function, and interface has a content-addressed hash computed from its structure. If two hashes match, the interfaces are compatible.

```
Actor A                              Actor B
   │                                    │
   │ import-hashes:                     │ export-hashes:
   │ "math/ops" → a1b2c3d4...           │ "math/ops" → a1b2c3d4...
   │                                    │
   └──────────── hashes match ──────────┘
                    ✓ compatible!
```

## Design Principles

### 1. Structural Typing for Types

Type **names** are NOT part of the hash. Two types with the same structure have the same hash, regardless of what they're called:

```wit
record Point { x: s32, y: s32 }     // hash: abc123...
record Vec2  { x: s32, y: s32 }     // hash: abc123... (same!)
```

This enables structural compatibility: if you expect a `Point` and receive a `Vec2` with the same fields, they're compatible.

### 2. Field Names ARE Structural

Field and case names **are** part of the hash, because they affect how data is accessed:

```wit
record Point { x: s32, y: s32 }     // hash: abc123...
record Point { a: s32, b: s32 }     // hash: def456... (different!)
```

You access `.x` vs `.a` - that's semantically meaningful.

### 3. Nominal Binding at Interface Level

While types are structural, **interfaces** include their binding names:

```wit
interface A {
    type point = { x: s32, y: s32 }
    translate: func(p: point) -> point
}

interface B {
    type vec2 = { x: s32, y: s32 }
    translate: func(p: vec2) -> vec2
}
```

These have **different interface hashes** because the bindings differ (`point` vs `vec2`), even though the underlying type structure is the same.

### 4. Function Signatures Exclude Parameter Names

Parameter names are documentation, not semantics:

```wit
func add(a: s32, b: s32) -> s32    // hash: xyz789...
func add(x: s32, y: s32) -> s32    // hash: xyz789... (same!)
```

What matters is the types, not what you call the parameters.

## Hash Construction

### Primitives

Primitives have fixed, well-known hashes:

| Type    | Hash (prefix) |
|---------|---------------|
| bool    | 0x0001...     |
| u8      | 0x0002...     |
| u16     | 0x0003...     |
| ...     | ...           |
| string  | 0x000d...     |

### Compound Types

Compound types hash their components:

```
hash(list<T>)        = sha256(TAG_LIST || hash(T))
hash(option<T>)      = sha256(TAG_OPTION || hash(T))
hash(result<T, E>)   = sha256(TAG_RESULT || hash(T) || hash(E))
hash(tuple<T1, T2>)  = sha256(TAG_TUPLE || count || hash(T1) || hash(T2))
```

### Records and Variants

Records and variants hash their fields/cases in **sorted order** (by name) for canonical ordering:

```
hash(record { y: s32, x: s32 })
  = sha256(TAG_RECORD || 2 || "x" || hash(s32) || "y" || hash(s32))

// Same as hash(record { x: s32, y: s32 }) - order in source doesn't matter
```

Type names are NOT included:

```
hash(record Point { x: s32 })  ==  hash(record Vec2 { x: s32 })
```

### Functions

Functions hash parameter types and result types (names excluded):

```
hash(func add(a: s32, b: s32) -> s32)
  = sha256(TAG_FUNCTION || 2 || hash(s32) || hash(s32) || 1 || hash(s32))
                          ^params                         ^results
```

### Interfaces

Interfaces hash their name plus sorted bindings:

```
hash(interface Math { add: func..., mul: func... })
  = sha256(TAG_INTERFACE || "Math"
           || type_bindings...   // sorted by name
           || func_bindings...)  // sorted by name
```

Bindings are `(name, hash)` pairs, so the binding names ARE part of the interface hash.

### Recursive Types

Recursive types use a self-reference placeholder:

```wit
variant sexpr {
    sym(string),
    lst(list<sexpr>),  // recursive!
}
```

The hash uses `HASH_SELF_REF` for the recursive reference:

```
hash(sexpr) = sha256(TAG_VARIANT || 2
                     || "lst" || hash(list<SELF_REF>)
                     || "sym" || hash(string))
```

This produces a stable hash even for recursive structures.

## Metadata Format

Package metadata includes interface hashes:

```
record package-metadata {
    imports: list<function-sig>,
    exports: list<function-sig>,
    import-hashes: list<interface-hash>,   // NEW
    export-hashes: list<interface-hash>,   // NEW
}

record interface-hash {
    name: string,                          // e.g., "theater:simple/runtime"
    hash: tuple<u64, u64, u64, u64>,       // SHA-256 as 4 u64s
}
```

## Validation Flow

### At Compile Time

1. Parse wit+ interface definitions
2. Compute hash for each type, function, interface
3. Embed hashes in package metadata

### At Runtime

1. Load package, call `__pack_types` to get metadata
2. Decode `import-hashes` and `export-hashes`
3. For each import, check if provider's export hash matches
4. Mismatch → error with details

```rust
let metadata = decode_metadata_with_hashes(&bytes)?;

for import in &metadata.import_hashes {
    let provider_hash = get_provider_export_hash(&import.name)?;
    if import.hash != provider_hash {
        return Err(InterfaceMismatch {
            interface: import.name.clone(),
            expected: import.hash,
            got: provider_hash,
        });
    }
}
```

## API Reference

### pack-abi (Guest Side)

```rust
use pack_abi::{
    TypeHash,
    hash_list, hash_option, hash_result, hash_tuple,
    hash_record, hash_variant, hash_function, hash_interface,
    Binding,
};

// Compute a record hash
let point_hash = hash_record(&[
    ("x", HASH_S32),
    ("y", HASH_S32),
]);

// Compute an interface hash
let math_hash = hash_interface(
    "math/ops",
    &[],  // type bindings
    &[
        Binding { name: "add", hash: add_func_hash },
        Binding { name: "mul", hash: mul_func_hash },
    ],
);
```

### pack (Host Side)

```rust
use pack::{decode_metadata_with_hashes, MetadataWithHashes, InterfaceHash};

let metadata: MetadataWithHashes = decode_metadata_with_hashes(&bytes)?;

// Check import hashes
for import in &metadata.import_hashes {
    println!("Import: {} -> {}", import.name, import.hash);
}

// Check export hashes
for export in &metadata.export_hashes {
    println!("Export: {} -> {}", export.name, export.hash);
}
```

## Benefits

1. **O(1) Compatibility Check**: Hash comparison instead of structural traversal
2. **Precise Diffs**: Merkle tree structure shows exactly where incompatibility lies
3. **Structural Sharing**: Same type structure = same hash, regardless of name
4. **Content Addressing**: Types become cacheable, distributable by hash
5. **Versioning**: Same interface name + different hash = different version

## Future Directions

- **Hash-based type registry**: Global cache of type definitions by hash
- **Distributed type checking**: Verify compatibility across network boundaries
- **Lazy type resolution**: Send hash first, fetch definition only if needed
- **Interface evolution rules**: Define when hash changes are backwards-compatible
