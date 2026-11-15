use serde::Deserialize;
use std::collections::HashSet;

#[derive(Deserialize)]
pub struct ParameterList {
    parameters: Vec<ParameterRow>,
}

#[derive(Deserialize)]
struct ParameterRow {
    name: String,
}

pub struct ParameterNames {
    pub index: usize,
    pub size: String,
    pub gap: String,
}

impl ParameterNames {
    pub fn allocate(existing: &ParameterList) -> Self {
        let occupied: HashSet<String> = existing
            .parameters
            .iter()
            .map(|row| row.name.to_ascii_lowercase())
            .collect();
        let mut index = 1;
        let prefix = loop {
            let prefix = format!("honeycomb_{index}");
            let taken = ["size", "gap"]
                .iter()
                .any(|suffix| occupied.contains(&format!("{prefix}_{suffix}")));
            if !taken {
                break prefix;
            }
            index += 1;
        };

        Self {
            index,
            size: format!("{prefix}_size"),
            gap: format!("{prefix}_gap"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_skip_collisions_in_either_parameter_slot() {
        let list: ParameterList = serde_json::from_str(
            r#"{"count":3,"cycles":[],"parameters":[
                {"name":"honeycomb_1_size","source":"user"},
                {"name":"HONEYCOMB_2_GAP","source":"feature"},
                {"name":"unrelated","source":"user"}
            ]}"#,
        )
        .unwrap();
        let names = ParameterNames::allocate(&list);
        assert_eq!(names.index, 3);
        assert_eq!(names.size, "honeycomb_3_size");
        assert_eq!(names.gap, "honeycomb_3_gap");
    }

    #[test]
    fn unexpected_list_response_fails_instead_of_assuming_the_design_is_empty() {
        assert!(serde_json::from_str::<ParameterList>(r#"{"count":2}"#).is_err());
        assert!(serde_json::from_str::<ParameterList>(r#"{"parameters":[{}]}"#).is_err());
        let empty = serde_json::from_str(r#"{"parameters":[]}"#).unwrap();
        assert_eq!(ParameterNames::allocate(&empty).size, "honeycomb_1_size");
    }
}
