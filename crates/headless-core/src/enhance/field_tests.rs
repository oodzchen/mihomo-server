use super::field::use_sort;
use serde_yaml_ng::{Mapping, Value};

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, reason = "tests assert by panicking")]
mod use_sort_tests {
    use super::{Mapping, Value, use_sort};

    #[test]
    fn every_key_survives_sorting() {
        let mut config = Mapping::new();
        for key in ["rules", "mode", "proxies", "not-a-known-field", "log-level"] {
            config.insert(Value::from(key), Value::from(key));
        }
        let expected = config.len();

        let sorted = use_sort(config);

        assert_eq!(sorted.len(), expected, "sorting must not drop or invent keys");
        for key in ["rules", "mode", "proxies", "not-a-known-field", "log-level"] {
            assert_eq!(sorted.get(Value::from(key)), Some(&Value::from(key)));
        }
    }

    #[test]
    fn the_bulky_list_fields_are_written_last() {
        // Bulky list fields go last so the top of the file stays readable.
        let mut config = Mapping::new();
        config.insert(Value::from("rules"), Value::from("rules"));
        config.insert(Value::from("mode"), Value::from("rule"));

        let sorted = use_sort(config);
        let order: Vec<_> = sorted.keys().filter_map(Value::as_str).collect();

        let mode = order.iter().position(|key| *key == "mode");
        let rules = order.iter().position(|key| *key == "rules");
        assert!(mode < rules, "expected mode before rules, got {order:?}");
    }

    #[test]
    fn sorting_is_stable_when_applied_twice() {
        let mut config = Mapping::new();
        for key in ["proxies", "mode", "unknown-key", "rules"] {
            config.insert(Value::from(key), Value::from(1));
        }

        let once = use_sort(config);
        let twice = use_sort(once.clone());

        assert_eq!(once.keys().collect::<Vec<_>>(), twice.keys().collect::<Vec<_>>());
    }
}
