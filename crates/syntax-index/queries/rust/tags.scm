(struct_item name: (type_identifier) @name) @definition.class
(enum_item name: (type_identifier) @name) @definition.class
(union_item name: (type_identifier) @name) @definition.class
(trait_item name: (type_identifier) @name) @definition.interface
(type_item name: (type_identifier) @name) @definition.type
(associated_type name: (type_identifier) @name) @definition.type
(function_item name: (identifier) @name) @definition.function
(function_signature_item name: (identifier) @name) @definition.function
(const_item name: (identifier) @name) @definition.constant
(static_item name: (identifier) @name) @definition.constant
(enum_variant name: (identifier) @name) @definition.variant
(field_declaration name: (field_identifier) @name) @definition.field
(mod_item name: (identifier) @name) @definition.module
(macro_definition name: (identifier) @name) @definition.macro

(call_expression function: (identifier) @name) @reference.call
(call_expression function: (field_expression field: (field_identifier) @name)) @reference.call
(call_expression function: (scoped_identifier name: (identifier) @name)) @reference.call
(use_declaration argument: (_) @name) @reference.import
(mod_item name: (identifier) @name !body) @reference.import
(impl_item
  trait: [(type_identifier) @parent
          (scoped_type_identifier name: (type_identifier) @parent)
          (generic_type type: (type_identifier) @parent)]
  type: [(type_identifier) @name
         (generic_type type: (type_identifier) @name)]) @reference.implementation
