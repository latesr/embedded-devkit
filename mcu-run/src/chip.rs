//! The chip description a run uses.

use crate::Error;
use probe_rs::Target;
use probe_rs::config::Registry;
use std::path::Path;

/// The built-in chip descriptions, with the family in `description` loaded over them when one is given. A
/// loaded family replaces the built-in family of the same name.
pub(crate) fn registry(description: Option<&Path>) -> Result<Registry, Error> {
    crate::register_espressif();
    let mut registry = Registry::from_builtin_families();
    if let Some(path) = description {
        let yaml = std::fs::read_to_string(path).map_err(|error| Error::ChipDescription {
            path: path.to_path_buf(),
            error: error.to_string(),
        })?;
        registry
            .add_target_family_from_yaml(&yaml)
            .map_err(|error| Error::ChipDescription {
                path: path.to_path_buf(),
                error: error.to_string(),
            })?;
    }
    Ok(registry)
}

/// Removes from the chip named `chip` every flash algorithm that covers part of the range of the algorithm named
/// `preferred`, so that probe-rs writes that range with `preferred`. Algorithms for other ranges stay. Resolve the
/// chip again with [`target`] after the call.
///
/// Fails with [`Error::UnknownFlashAlgorithm`] when the chip does not have an algorithm named `preferred`.
pub(crate) fn prefer_flash_algorithm(
    registry: &mut Registry,
    chip: &str,
    preferred: &str,
) -> Result<(), Error> {
    // probe-rs builds a flash region from the first algorithm that covers an address the memory map does not, when
    // it resolves the chip. The algorithms must leave the chip's list before that, or the region keeps the range of
    // an algorithm that is gone.
    let resolved = target(registry, chip)?;
    let range = resolved
        .flash_algorithms
        .iter()
        .find(|algorithm| algorithm.name == preferred)
        .map(|algorithm| algorithm.flash_properties.address_range.clone())
        .ok_or_else(|| Error::UnknownFlashAlgorithm {
            chip: resolved.name.clone(),
            algorithm: preferred.to_owned(),
        })?;
    let competing: Vec<&str> = resolved
        .flash_algorithms
        .iter()
        .filter(|algorithm| {
            let other = &algorithm.flash_properties.address_range;
            algorithm.name != preferred && other.start < range.end && range.start < other.end
        })
        .map(|algorithm| algorithm.name.as_str())
        .collect();
    let unknown = |error: String| Error::UnknownChip {
        chip: chip.to_owned(),
        error,
    };
    let mut family = registry
        .families()
        .iter()
        .find(|family| {
            family
                .variants
                .iter()
                .any(|variant| variant.name == resolved.name)
        })
        .cloned()
        .ok_or_else(|| unknown("no family holds the chip".to_owned()))?;
    for variant in family
        .variants
        .iter_mut()
        .filter(|variant| variant.name == resolved.name)
    {
        variant
            .flash_algorithms
            .retain(|name| !competing.contains(&name.as_str()));
    }
    registry
        .add_target_family(family)
        .map_err(|error| unknown(error.to_string()))?;
    Ok(())
}

/// The chip named `chip` in the registry.
pub(crate) fn target(registry: &Registry, chip: &str) -> Result<Target, Error> {
    registry
        .get_target_by_name(chip)
        .map_err(|error| Error::UnknownChip {
            chip: chip.to_owned(),
            error: error.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NUCLEO: &str = "mx25uw25645g_stm32h7s38-nucleo";
    const DK: &str = "mx66uw1g45g_stm32h7s78-dk";

    const CHIP: &str = "STM32H7S3L8";

    /// The chip, resolved after `preferred` is applied to `registry`.
    fn preferring(mut registry: Registry, preferred: &str) -> Target {
        prefer_flash_algorithm(&mut registry, CHIP, preferred).unwrap();
        target(&registry, CHIP).unwrap()
    }

    fn names(target: &Target) -> Vec<&str> {
        target
            .flash_algorithms
            .iter()
            .map(|algorithm| algorithm.name.as_str())
            .collect()
    }

    #[test]
    fn the_preferred_algorithm_replaces_the_one_that_covers_the_same_flash() {
        let target = preferring(registry(None).unwrap(), NUCLEO);
        assert_eq!(
            names(&target),
            ["stm32h7rx_64k", NUCLEO, "mx66uw1g45g_stm32h7s78-dk-xspim1"]
        );
    }

    #[test]
    fn the_flash_region_is_the_preferred_algorithm_range() {
        let target = preferring(registry(None).unwrap(), NUCLEO);
        let regions: Vec<_> = target
            .memory_map
            .iter()
            .filter_map(|region| match region {
                probe_rs::config::MemoryRegion::Nvm(nvm) if nvm.range.contains(&0x7000_0000) => {
                    Some((nvm.range.start, nvm.range.end))
                }
                _ => None,
            })
            .collect();
        assert_eq!(regions, [(0x7000_0000, 0x7200_0000)]);
    }

    #[test]
    fn an_algorithm_that_ends_or_starts_at_the_preferred_range_stays() {
        let mut registry = registry(None).unwrap();
        let mut family = registry
            .families()
            .iter()
            .find(|family| family.variants.iter().any(|variant| variant.name == CHIP))
            .unwrap()
            .clone();
        let algorithm = |name: &str| {
            family
                .flash_algorithms
                .iter()
                .find(|algorithm| algorithm.name == name)
                .unwrap()
                .clone()
        };
        let nucleo = algorithm(NUCLEO).flash_properties.address_range;
        let mut below = algorithm(DK);
        below.name = "below".to_owned();
        below.flash_properties.address_range = nucleo.start - 0x1000..nucleo.start;
        let mut above = below.clone();
        above.name = "above".to_owned();
        above.flash_properties.address_range = nucleo.end..nucleo.end + 0x1000;
        family.flash_algorithms.push(below);
        family.flash_algorithms.push(above);
        let chip = family
            .variants
            .iter_mut()
            .find(|variant| variant.name == CHIP)
            .unwrap();
        chip.flash_algorithms.push("below".to_owned());
        chip.flash_algorithms.push("above".to_owned());
        registry.add_target_family(family).unwrap();
        let target = preferring(registry, NUCLEO);
        assert_eq!(
            names(&target),
            [
                "stm32h7rx_64k",
                NUCLEO,
                "mx66uw1g45g_stm32h7s78-dk-xspim1",
                "below",
                "above"
            ]
        );
    }

    #[test]
    fn an_unknown_algorithm_is_named_with_the_chip() {
        let mut registry = registry(None).unwrap();
        let error = prefer_flash_algorithm(&mut registry, CHIP, "no-such-algorithm").unwrap_err();
        assert_eq!(
            error,
            Error::UnknownFlashAlgorithm {
                chip: "STM32H7S3L8".to_owned(),
                algorithm: "no-such-algorithm".to_owned(),
            }
        );
        assert_eq!(
            error.to_string(),
            "STM32H7S3L8 has no flash algorithm named no-such-algorithm"
        );
        assert_eq!(names(&target(&registry, CHIP).unwrap()).len(), 4);
    }
}
