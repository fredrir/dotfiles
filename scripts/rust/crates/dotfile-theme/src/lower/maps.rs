use std::collections::HashMap;

use dotfile_source::ByteRange;
use dotfile_syntax::{Block, File, Value};

use crate::model::{
    CatppuccinEntry, CatppuccinMap, EzaCategory, EzaExtension, EzaMap, GtkEntry, GtkMap, HexKey,
    KdeGroupEntry, KdeMap, KdeRoleEntry, ObsidianDerived, ObsidianMap, ObsidianValue,
    ObsidianVariable, Spanned, ThemeReference,
};

use super::common::{Context, DecimalConstraint};

pub(super) fn lower_catppuccin(context: &mut Context<'_>, file: File<'_>) -> Option<CatppuccinMap> {
    let span = file.range();
    let colors = exact_root_block(context, file, "colors")?;
    let mut entries = Vec::new();
    let mut keys = HashMap::new();
    for record in colors.entries() {
        let Some(named) = context.plain_named(record) else {
            continue;
        };
        if named.name() != Some("entry") {
            context.schema(
                named.range(),
                "catppuccin colors accepts only `entry` records",
            );
            continue;
        }
        let Some(block) = context.require_block(named, "entry") else {
            continue;
        };
        let fields = lower_key_reference_fields(context, block, "palette", "catppuccin entry");
        let Some(key) = fields.key else {
            continue;
        };
        let unique_key =
            context.track_unique(&mut keys, &key.value, key.span, "catppuccin color key");
        if !is_plain_hex(&key.value) {
            context.schema(
                key.span,
                "catppuccin entry key must be six lowercase hexadecimal digits",
            );
            continue;
        }
        let Some(palette) = fields.reference else {
            continue;
        };
        if unique_key {
            entries.push(CatppuccinEntry {
                key: Spanned::new(HexKey(key.value), key.span),
                palette,
                span: named.range(),
            });
        }
    }
    Some(CatppuccinMap { entries, span })
}

pub(super) fn lower_eza(context: &mut Context<'_>, file: File<'_>) -> Option<EzaMap> {
    let span = file.range();
    let categories_block = exact_root_block(context, file, "categories")?;
    let mut categories = Vec::new();
    let mut names = HashMap::new();
    let mut extension_owners: HashMap<String, Vec<ByteRange>> = HashMap::new();
    for record in categories_block.entries() {
        let Some(named) = context.plain_named(record) else {
            continue;
        };
        if named.name() != Some("category") {
            context.schema(
                named.range(),
                "Eza categories accepts only `category` records",
            );
            continue;
        }
        let Some(block) = context.require_block(named, "category") else {
            continue;
        };
        let fields = lower_eza_category(context, block);
        let extensions = fields.extensions;
        if let Some(extensions) = &extensions {
            for extension in extensions {
                extension_owners
                    .entry(extension.value.as_str().to_owned())
                    .or_default()
                    .push(extension.span);
            }
        }
        let Some(name) = fields.name else {
            continue;
        };
        let unique_name =
            context.track_unique(&mut names, &name.value, name.span, "Eza category name");
        let Some(extensions) = extensions else {
            continue;
        };
        let mut unique_extensions = true;
        for extension in &extensions {
            if extension_owners
                .get(extension.value.as_str())
                .is_some_and(|owners| owners.len() > 1)
            {
                unique_extensions = false;
            }
        }
        if unique_name && unique_extensions {
            categories.push(EzaCategory {
                name,
                extensions,
                span: named.range(),
            });
        }
    }
    let mut conflicts: Vec<_> = extension_owners
        .into_iter()
        .filter(|(_, origins)| origins.len() > 1)
        .collect();
    conflicts.sort_by_key(|(_, origins)| origins[1].start());
    for (extension, origins) in conflicts {
        let primary = origins[1];
        let related: Vec<_> = origins
            .into_iter()
            .enumerate()
            .filter_map(|(index, origin)| (index != 1).then_some(origin))
            .collect();
        context.map_conflict(
            primary,
            &related,
            format!("Eza extension `{extension}` belongs to more than one category"),
        );
    }
    Some(EzaMap { categories, span })
}

pub(super) fn lower_gtk(context: &mut Context<'_>, file: File<'_>) -> Option<GtkMap> {
    let span = file.range();
    let colors = exact_root_block(context, file, "colors")?;
    let mut entries = Vec::new();
    let mut keys = HashMap::new();
    for record in colors.entries() {
        let Some(named) = context.plain_named(record) else {
            continue;
        };
        if named.name() != Some("entry") {
            context.schema(named.range(), "GTK colors accepts only `entry` records");
            continue;
        }
        let Some(block) = context.require_block(named, "entry") else {
            continue;
        };
        let fields = lower_key_reference_fields(context, block, "role", "GTK entry");
        let Some(key) = fields.key else {
            continue;
        };
        let unique_key = context.track_unique(&mut keys, &key.value, key.span, "GTK external key");
        let Some(role) = fields.reference else {
            continue;
        };
        if unique_key {
            entries.push(GtkEntry {
                key,
                role,
                span: named.range(),
            });
        }
    }
    Some(GtkMap { entries, span })
}

pub(super) fn lower_kde(context: &mut Context<'_>, file: File<'_>) -> Option<KdeMap> {
    let span = file.range();
    let mut seen = HashMap::new();
    let mut groups = None;
    let mut foregrounds = None;
    let mut selection_foregrounds = None;

    for entry in file.entries() {
        let Some(named) = context.plain_named(entry) else {
            continue;
        };
        let Some(name) = named.name() else {
            continue;
        };
        if !matches!(name, "groups" | "foregrounds" | "selection-foregrounds") {
            context.schema(
                named.range(),
                format!("unknown KDE map root field `{name}`"),
            );
            continue;
        }
        if !context.track_unique(
            &mut seen,
            name,
            named.name_range().unwrap_or_else(|| named.range()),
            "KDE map block",
        ) {
            continue;
        }
        let Some(block) = context.require_block(named, name) else {
            continue;
        };
        match name {
            "groups" => groups = Some(lower_kde_groups(context, block)),
            "foregrounds" => foregrounds = Some(lower_kde_roles(context, block, name)),
            "selection-foregrounds" => {
                selection_foregrounds = Some(lower_kde_roles(context, block, name));
            }
            _ => unreachable!(),
        }
    }
    for name in ["groups", "foregrounds", "selection-foregrounds"] {
        if !seen.contains_key(name) {
            context.missing(span, name);
        }
    }
    Some(KdeMap {
        groups: groups?,
        foregrounds: foregrounds?,
        selection_foregrounds: selection_foregrounds?,
        span,
    })
}

pub(super) fn lower_obsidian(context: &mut Context<'_>, file: File<'_>) -> Option<ObsidianMap> {
    let span = file.range();
    let mut seen = HashMap::new();
    let mut source = None;
    let mut variables = None;
    for entry in file.entries() {
        let Some(named) = context.plain_named(entry) else {
            continue;
        };
        let Some(name) = named.name() else {
            continue;
        };
        if !matches!(name, "derived" | "variables") {
            context.schema(
                named.range(),
                format!("unknown Obsidian map root field `{name}`"),
            );
            continue;
        }
        if !context.track_unique(
            &mut seen,
            name,
            named.name_range().unwrap_or_else(|| named.range()),
            "Obsidian map block",
        ) {
            continue;
        }
        let Some(block) = context.require_block(named, name) else {
            continue;
        };
        match name {
            "derived" => source = lower_derived(context, block),
            "variables" => variables = Some(lower_variables(context, block)),
            _ => unreachable!(),
        }
    }
    if !seen.contains_key("derived") {
        context.missing(span, "derived");
    }
    if !seen.contains_key("variables") {
        context.missing(span, "variables");
    }
    Some(ObsidianMap {
        source: source?,
        variables: variables?,
        span,
    })
}

fn exact_root_block<'a>(
    context: &mut Context<'_>,
    file: File<'a>,
    expected: &str,
) -> Option<Block<'a>> {
    let mut result = None;
    let mut origins = Vec::new();
    for entry in file.entries() {
        let Some(named) = context.plain_named(entry) else {
            continue;
        };
        let Some(name) = named.name() else {
            continue;
        };
        if name != expected {
            context.schema(
                named.range(),
                format!("unknown root field `{name}`; expected `{expected}`"),
            );
            continue;
        }
        let name_span = named.name_range().unwrap_or_else(|| named.range());
        if !origins.is_empty() {
            context.duplicate(
                name_span,
                &origins,
                format!("duplicate `{expected}` map block"),
            );
            origins.push(name_span);
            continue;
        }
        origins.push(name_span);
        result = context.require_block(named, expected);
    }
    if origins.is_empty() {
        context.missing(file.range(), expected);
    }
    result
}

struct EzaCategoryFields {
    name: Option<Spanned<String>>,
    extensions: Option<Vec<Spanned<EzaExtension>>>,
}

fn lower_eza_category(context: &mut Context<'_>, block: Block<'_>) -> EzaCategoryFields {
    let mut seen = HashMap::new();
    let mut name = None;
    let mut extensions = None;
    for entry in block.entries() {
        let Some(named) = context.plain_named(entry) else {
            continue;
        };
        let Some(field) = named.name() else {
            continue;
        };
        if !matches!(field, "name" | "extensions") {
            context.schema(
                named.range(),
                format!("unknown Eza category field `{field}`"),
            );
            continue;
        }
        if !context.track_unique(
            &mut seen,
            field,
            named.name_range().unwrap_or_else(|| named.range()),
            "Eza category field",
        ) {
            continue;
        }
        let Some(value) = context.require_value(named, field) else {
            continue;
        };
        match field {
            "name" => {
                name = context
                    .reference(value, "category.name")
                    .map(|reference| Spanned::new(reference.name, reference.span));
            }
            "extensions" => extensions = lower_extensions(context, value),
            _ => unreachable!(),
        }
    }
    if !seen.contains_key("name") {
        context.missing(block.range(), "category.name");
    }
    if !seen.contains_key("extensions") {
        context.missing(block.range(), "category.extensions");
    }
    EzaCategoryFields { name, extensions }
}

fn lower_extensions(
    context: &mut Context<'_>,
    value: Value<'_>,
) -> Option<Vec<Spanned<EzaExtension>>> {
    let Value::List(list) = value else {
        context.schema(
            value.range(),
            "category.extensions must be a list of strings",
        );
        return None;
    };
    let values = list.values();
    if values.is_empty() {
        if context.directly_poisoned(list.node_id()) {
            return None;
        }
        context.schema(list.range(), "category.extensions must not be empty");
        return None;
    }
    let mut extensions = Vec::new();
    let mut seen = HashMap::new();
    for value in values {
        let Some(extension) = context.literal_string(value, "category.extensions item") else {
            continue;
        };
        let unique_extension =
            context.track_unique(&mut seen, &extension.value, extension.span, "Eza extension");
        if !is_extension(&extension.value) {
            context.schema(
                extension.span,
                "Eza extension must match `[a-z0-9][a-z0-9_+-]*`",
            );
            continue;
        }
        if unique_extension {
            extensions.push(Spanned::new(EzaExtension(extension.value), extension.span));
        }
    }
    Some(extensions)
}

fn lower_key_role_record(
    context: &mut Context<'_>,
    block: Block<'_>,
    label: &str,
) -> KeyReferenceFields {
    lower_key_reference_fields(context, block, "role", label)
}

struct KeyReferenceFields {
    key: Option<Spanned<String>>,
    reference: Option<ThemeReference>,
}

fn lower_key_reference_fields(
    context: &mut Context<'_>,
    block: Block<'_>,
    reference_field: &str,
    label: &str,
) -> KeyReferenceFields {
    let mut seen = HashMap::new();
    let mut key = None;
    let mut reference = None;
    for entry in block.entries() {
        let Some(named) = context.plain_named(entry) else {
            continue;
        };
        let Some(field) = named.name() else {
            continue;
        };
        if field != "key" && field != reference_field {
            context.schema(named.range(), format!("unknown {label} field `{field}`"));
            continue;
        }
        if !context.track_unique(
            &mut seen,
            field,
            named.name_range().unwrap_or_else(|| named.range()),
            &format!("{label} field"),
        ) {
            continue;
        }
        let Some(value) = context.require_value(named, field) else {
            continue;
        };
        if field == "key" {
            key = context.literal_string(value, &format!("{label}.key"));
        } else {
            reference = context.reference(value, &format!("{label}.{reference_field}"));
        }
    }
    if !seen.contains_key("key") {
        context.missing(block.range(), &format!("{label}.key"));
    }
    if !seen.contains_key(reference_field) {
        context.missing(block.range(), &format!("{label}.{reference_field}"));
    }
    KeyReferenceFields { key, reference }
}

fn lower_kde_groups(context: &mut Context<'_>, block: Block<'_>) -> Vec<KdeGroupEntry> {
    let mut entries = Vec::new();
    let mut keys = HashMap::new();
    for record in block.entries() {
        let Some(named) = context.plain_named(record) else {
            continue;
        };
        if named.name() != Some("entry") {
            context.schema(named.range(), "KDE groups accepts only `entry` records");
            continue;
        }
        let Some(child) = context.require_block(named, "entry") else {
            continue;
        };
        let fields = lower_kde_group_record(context, child);
        let Some(key) = fields.key else {
            continue;
        };
        let unique_key =
            context.track_unique(&mut keys, &key.value, key.span, "KDE group external key");
        let Some(roles) = fields.roles else {
            continue;
        };
        if unique_key {
            entries.push(KdeGroupEntry {
                key,
                roles,
                span: named.range(),
            });
        }
    }
    entries
}

struct KdeGroupFields {
    key: Option<Spanned<String>>,
    roles: Option<[ThemeReference; 2]>,
}

fn lower_kde_group_record(context: &mut Context<'_>, block: Block<'_>) -> KdeGroupFields {
    let mut seen = HashMap::new();
    let mut key = None;
    let mut roles = None;
    for entry in block.entries() {
        let Some(named) = context.plain_named(entry) else {
            continue;
        };
        let Some(field) = named.name() else {
            continue;
        };
        if !matches!(field, "key" | "roles") {
            context.schema(named.range(), format!("unknown KDE group field `{field}`"));
            continue;
        }
        if !context.track_unique(
            &mut seen,
            field,
            named.name_range().unwrap_or_else(|| named.range()),
            "KDE group field",
        ) {
            continue;
        }
        let Some(value) = context.require_value(named, field) else {
            continue;
        };
        match field {
            "key" => key = context.literal_string(value, "KDE group key"),
            "roles" => roles = lower_two_references(context, value, "KDE group roles"),
            _ => unreachable!(),
        }
    }
    if !seen.contains_key("key") {
        context.missing(block.range(), "KDE group key");
    }
    if !seen.contains_key("roles") {
        context.missing(block.range(), "KDE group roles");
    }
    KdeGroupFields { key, roles }
}

fn lower_two_references(
    context: &mut Context<'_>,
    value: Value<'_>,
    field: &str,
) -> Option<[ThemeReference; 2]> {
    let Value::List(list) = value else {
        context.schema(value.range(), format!("{field} must be a list"));
        return None;
    };
    let values = list.values();
    let valid_cardinality = values.len() == 2;
    if !valid_cardinality {
        if context.directly_poisoned(list.node_id()) {
            // The parser owns the incomplete list shape, but any complete
            // elements still receive their independent value checks below.
        } else {
            context.schema(
                list.range(),
                format!("{field} must contain exactly two references"),
            );
        }
    }
    let references: Vec<_> = values
        .into_iter()
        .map(|value| context.reference(value, field))
        .collect();
    if !valid_cardinality {
        return None;
    }
    let mut references = references.into_iter();
    let first = references.next().flatten()?;
    let second = references.next().flatten()?;
    Some([first, second])
}

fn lower_kde_roles(
    context: &mut Context<'_>,
    block: Block<'_>,
    container: &str,
) -> Vec<KdeRoleEntry> {
    let mut entries = Vec::new();
    let mut keys = HashMap::new();
    for record in block.entries() {
        let Some(named) = context.plain_named(record) else {
            continue;
        };
        if named.name() != Some("entry") {
            context.schema(
                named.range(),
                format!("KDE `{container}` accepts only `entry` records"),
            );
            continue;
        }
        let Some(child) = context.require_block(named, "entry") else {
            continue;
        };
        let fields = lower_key_role_record(context, child, "KDE entry");
        let Some(key) = fields.key else {
            continue;
        };
        let unique_key = context.track_unique(
            &mut keys,
            &key.value,
            key.span,
            &format!("KDE {container} external key"),
        );
        let Some(role) = fields.reference else {
            continue;
        };
        if unique_key {
            entries.push(KdeRoleEntry {
                key,
                role,
                span: named.range(),
            });
        }
    }
    entries
}

fn lower_derived(context: &mut Context<'_>, block: Block<'_>) -> Option<ThemeReference> {
    let mut seen = HashMap::new();
    let mut source = None;
    for entry in block.entries() {
        let Some(named) = context.plain_named(entry) else {
            continue;
        };
        let Some(field) = named.name() else {
            continue;
        };
        if field != "source" {
            context.schema(
                named.range(),
                format!("unknown Obsidian derived field `{field}`"),
            );
            continue;
        }
        if !context.track_unique(
            &mut seen,
            field,
            named.name_range().unwrap_or_else(|| named.range()),
            "Obsidian derived field",
        ) {
            continue;
        }
        source = context
            .require_value(named, field)
            .and_then(|value| context.reference(value, "derived.source"));
    }
    if !seen.contains_key("source") {
        context.missing(block.range(), "derived.source");
    }
    source
}

fn lower_variables(context: &mut Context<'_>, block: Block<'_>) -> Vec<ObsidianVariable> {
    let mut variables = Vec::new();
    let mut keys = HashMap::new();
    for record in block.entries() {
        let Some(named) = context.plain_named(record) else {
            continue;
        };
        if named.name() != Some("variable") {
            context.schema(
                named.range(),
                "Obsidian variables accepts only `variable` records",
            );
            continue;
        }
        let Some(child) = context.require_block(named, "variable") else {
            continue;
        };
        let fields = lower_variable(context, child);
        let Some(key) = fields.key else {
            continue;
        };
        let unique_key =
            context.track_unique(&mut keys, &key.value, key.span, "Obsidian variable key");
        let Some(value) = fields.value else {
            continue;
        };
        if fields.key_valid && unique_key {
            variables.push(ObsidianVariable {
                key,
                value,
                span: named.range(),
            });
        }
    }
    variables
}

struct ObsidianVariableFields {
    key: Option<Spanned<String>>,
    key_valid: bool,
    value: Option<ObsidianValue>,
}

fn lower_variable(context: &mut Context<'_>, block: Block<'_>) -> ObsidianVariableFields {
    let mut seen = HashMap::new();
    let mut key = None;
    let mut palette = None;
    let mut rgb = None;
    let mut color = None;
    let mut alpha = None;
    let mut derived = None;
    let mut literal = None;
    for entry in block.entries() {
        let Some(named) = context.plain_named(entry) else {
            continue;
        };
        let Some(field) = named.name() else {
            continue;
        };
        if !matches!(
            field,
            "key" | "palette" | "rgb" | "color" | "alpha" | "derived" | "literal"
        ) {
            context.schema(
                named.range(),
                format!("unknown Obsidian variable field `{field}`"),
            );
            continue;
        }
        if !context.track_unique(
            &mut seen,
            field,
            named.name_range().unwrap_or_else(|| named.range()),
            "Obsidian variable field",
        ) {
            continue;
        }
        let Some(value) = context.require_value(named, field) else {
            continue;
        };
        match field {
            "key" => key = context.literal_string(value, "variable.key"),
            "palette" => palette = context.reference(value, "variable.palette"),
            "rgb" => rgb = context.reference(value, "variable.rgb"),
            "color" => color = context.reference(value, "variable.color"),
            "alpha" => {
                alpha = context.canonical_decimal(
                    value,
                    "variable.alpha",
                    DecimalConstraint::ZeroToOne,
                );
            }
            "derived" => derived = lower_obsidian_derived(context, value),
            "literal" => literal = context.literal_string(value, "variable.literal"),
            _ => unreachable!(),
        }
    }
    if !seen.contains_key("key") {
        context.missing(block.range(), "variable.key");
    }
    let key_valid = key.as_ref().is_some_and(|key_value| {
        let valid = is_css_key(&key_value.value);
        if !valid {
            context.schema(
                key_value.span,
                "Obsidian variable key must be a nonempty CSS custom-property name",
            );
        }
        valid
    });

    let shape_count = ["palette", "rgb", "color", "derived", "literal"]
        .into_iter()
        .filter(|field| seen.contains_key(*field))
        .count();
    let alpha_is_legal = !seen.contains_key("alpha") || seen.contains_key("color");
    if !alpha_is_legal {
        context.schema(block.range(), "alpha is valid only with the color shape");
    }
    if shape_count != 1 {
        context.schema(
            block.range(),
            "Obsidian variable must declare exactly one value shape",
        );
        return ObsidianVariableFields {
            key,
            key_valid,
            value: None,
        };
    }

    let value = if seen.contains_key("palette") {
        if !alpha_is_legal {
            None
        } else {
            palette.map(ObsidianValue::Palette)
        }
    } else if seen.contains_key("rgb") {
        if !alpha_is_legal {
            None
        } else {
            rgb.map(ObsidianValue::Rgb)
        }
    } else if seen.contains_key("color") {
        if !seen.contains_key("alpha") {
            context.missing(block.range(), "variable.alpha");
            None
        } else {
            color
                .zip(alpha)
                .map(|(color, alpha)| ObsidianValue::Color { color, alpha })
        }
    } else if seen.contains_key("derived") {
        if !alpha_is_legal {
            None
        } else {
            derived.map(ObsidianValue::Derived)
        }
    } else {
        if !alpha_is_legal {
            None
        } else {
            literal.map(ObsidianValue::Literal)
        }
    };
    ObsidianVariableFields {
        key,
        key_valid,
        value,
    }
}

fn lower_obsidian_derived(
    context: &mut Context<'_>,
    value: Value<'_>,
) -> Option<Spanned<ObsidianDerived>> {
    let reference = context.reference(value, "variable.derived")?;
    let derived = match reference.name.as_str() {
        "accent_h" => ObsidianDerived::AccentH,
        "accent_s" => ObsidianDerived::AccentS,
        "accent_l" => ObsidianDerived::AccentL,
        "accent_hsl" => ObsidianDerived::AccentHsl,
        _ => {
            context.schema(
                reference.span,
                "derived value must be accent_h, accent_s, accent_l, or accent_hsl",
            );
            return None;
        }
    };
    Some(Spanned::new(derived, reference.span))
}

fn is_plain_hex(text: &str) -> bool {
    text.len() == 6
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_extension(text: &str) -> bool {
    let mut bytes = text.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_lowercase() || first.is_ascii_digit())
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'+' | b'-')
        })
}

fn is_css_key(text: &str) -> bool {
    let Some(rest) = text.strip_prefix("--") else {
        return false;
    };
    !rest.is_empty()
        && rest.chars().all(|scalar| {
            scalar.is_ascii_alphanumeric() || matches!(scalar, '_' | '-') || !scalar.is_ascii()
        })
}
