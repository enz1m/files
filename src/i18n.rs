// App message catalogs and locale-scoped display text.

use enzui_core::localization::{
    self, Arguments, Catalog, CatalogBuilder, Locale, Localizer, Value,
};
use std::sync::{LazyLock, OnceLock};

static STATIC_MESSAGES: LazyLock<Vec<(String, String)>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../assets/locales/static-messages.json"))
        .expect("embedded source-label index must be valid")
});

pub(crate) const ENGLISH: &str = include_str!("../assets/locales/en-US.ftl");
const GERMAN: &str = include_str!("../assets/locales/de.ftl");
const ARABIC: &str = include_str!("../assets/locales/ar.ftl");

static CATALOG: LazyLock<Catalog> = LazyLock::new(|| {
    let mut builder = CatalogBuilder::new();
    for (locale, resource) in [("en-US", ENGLISH), ("de", GERMAN), ("ar", ARABIC)] {
        builder
            .add_resource(locale, resource)
            .expect("embedded Files catalogs must be valid Fluent");
    }
    builder.build()
});

pub(crate) fn app_localizer() -> Localizer {
    Localizer::new(
        CATALOG.clone(),
        localization::preferred_locales(),
        Locale::default(),
    )
}

static ENGLISH_TEXT: OnceLock<Vec<String>> = OnceLock::new();
static GERMAN_TEXT: OnceLock<Vec<String>> = OnceLock::new();
static ARABIC_TEXT: OnceLock<Vec<String>> = OnceLock::new();
static PSEUDO_TEXT: OnceLock<Vec<String>> = OnceLock::new();

pub(crate) fn localizer() -> Localizer {
    localization::scoped().unwrap_or_else(app_localizer)
}

pub(crate) fn for_locale(locale: Locale) -> Localizer {
    Localizer::new(CATALOG.clone(), vec![locale], Locale::default())
}

// Fixed catalogs bound the cache; borrowed labels never refer to a temporary translation.
pub(crate) fn tr(source: &'static str) -> &'static str {
    let Ok(index) =
        STATIC_MESSAGES.binary_search_by(|(candidate, _)| candidate.as_str().cmp(source))
    else {
        return source;
    };
    let locale = localizer().locale().to_string();
    let (cache, tag) = if locale.eq_ignore_ascii_case("en-XA") {
        (&PSEUDO_TEXT, "en-XA")
    } else if locale == "de" || locale.starts_with("de-") {
        (&GERMAN_TEXT, "de")
    } else if locale == "ar" || locale.starts_with("ar-") {
        (&ARABIC_TEXT, "ar")
    } else {
        (&ENGLISH_TEXT, "en-US")
    };
    &cache.get_or_init(|| {
        let localizer = for_locale(tag.parse().expect("fixed locale tag"));
        STATIC_MESSAGES
            .iter()
            .map(|(_, id)| localizer.text(id))
            .collect()
    })[index]
}

pub(crate) fn text(source: &str) -> String {
    STATIC_MESSAGES
        .binary_search_by(|(candidate, _)| candidate.as_str().cmp(source))
        .map_or_else(
            |_| source.to_owned(),
            |index| localizer().text(&STATIC_MESSAGES[index].1),
        )
}

pub(crate) fn format(id: &str, values: &[(&str, String)]) -> String {
    let mut arguments = Arguments::new();
    for (name, value) in values {
        static NUMERIC: LazyLock<std::collections::BTreeMap<String, Vec<String>>> =
            LazyLock::new(|| {
                serde_json::from_str(include_str!("../assets/locales/numeric-arguments.json"))
                    .expect("embedded numeric arguments")
            });
        let numeric = NUMERIC
            .get(id)
            .is_some_and(|names| names.iter().any(|numeric| numeric == name));
        if let Some(number) = numeric.then(|| value.parse::<u64>().ok()).flatten() {
            arguments.set(*name, Value::from(number));
        } else {
            arguments.set(*name, value.as_str());
        }
    }
    localizer().text_with(id, &arguments)
}

pub(crate) fn count(id: &str, value: u64) -> String {
    let mut arguments = Arguments::new();
    arguments.set("count", Value::from(value));
    localizer().text_with(id, &arguments)
}

pub(crate) fn rtl() -> bool {
    localizer().locale().direction() == enzui_core::placement::Direction::RightToLeft
}

pub(crate) fn radio_labels(group: &'static str) -> &'static [&'static str] {
    static LABELS: LazyLock<std::collections::BTreeMap<(&str, &str), Vec<&'static str>>> =
        LazyLock::new(|| {
            let mut labels = std::collections::BTreeMap::new();
            for tag in ["en-US", "de", "ar", "en-XA"] {
                for_locale(tag.parse().expect("fixed locale tag")).scope(|| {
                    labels.insert(
                        (tag, "rename"),
                        crate::bulk_rename::BULK_RENAME_MODE_LABELS.map(tr).to_vec(),
                    );
                    labels.insert(
                        (tag, "behavior"),
                        crate::open_with::OPEN_BEHAVIOR_LABELS.map(tr).to_vec(),
                    );
                    labels.insert(
                        (tag, "type"),
                        crate::dialog::FILTER_TYPES
                            .map(super::model::SearchFilterType::label)
                            .to_vec(),
                    );
                    labels.insert(
                        (tag, "date"),
                        crate::dialog::FILTER_DATES
                            .map(super::model::SearchFilterDate::label)
                            .to_vec(),
                    );
                });
            }
            labels
        });
    let locale = localizer().locale().to_string();
    let tag = if locale.starts_with("de") {
        "de"
    } else if locale.starts_with("ar") {
        "ar"
    } else if locale == "en-XA" {
        "en-XA"
    } else {
        "en-US"
    };
    LABELS.get(&(tag, group)).expect("known radio group")
}

#[cfg(test)]
pub(crate) fn unisolated(text: &str) -> String {
    text.chars()
        .filter(|character| !matches!(character, '\u{2068}' | '\u{2069}'))
        .collect()
}
