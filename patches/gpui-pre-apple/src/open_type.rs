use core_foundation::{
    array::{
        CFArray, CFArrayAppendValue, CFArrayCreateMutable, CFMutableArrayRef, kCFTypeArrayCallBacks,
    },
    base::{CFRelease, TCFType, kCFAllocatorDefault},
    dictionary::{
        CFDictionaryCreate, kCFTypeDictionaryKeyCallBacks, kCFTypeDictionaryValueCallBacks,
    },
    number::CFNumber,
    string::CFString,
};
use core_foundation_sys::locale::CFLocaleCopyPreferredLanguages;
use core_text::font_descriptor::{
    TraitAccessors, kCTFontFamilyNameAttribute, kCTFontItalicTrait, kCTFontSlantTrait,
    kCTFontTraitsAttribute, kCTFontWeightTrait,
};
use core_text::{
    font::{CTFont, cascade_list_for_languages},
    font_descriptor::{
        CTFontDescriptor, CTFontDescriptorCreateWithAttributes, kCTFontCascadeListAttribute,
        kCTFontFeatureSettingsAttribute,
    },
};
use font_kit::font::Font as FontKitFont;
use gpui::{FontFallbacks, FontFeatures};
use objc2_core_text::{kCTFontOpenTypeFeatureTag, kCTFontOpenTypeFeatureValue};

pub fn apply_features_and_fallbacks(
    font: &mut FontKitFont,
    features: &FontFeatures,
    fallbacks: Option<&FontFallbacks>,
) -> anyhow::Result<()> {
    let descriptor = features_and_fallbacks_descriptor(font, features, fallbacks);
    let new_font = copy_with_descriptor(&font.native_font(), 0.0, &descriptor);
    *font = unsafe { font_kit::font::Font::from_native_font(&new_font) };
    Ok(())
}

/// The descriptor that adds `features` and the `fallbacks` cascade (then the system's
/// cascade for the user's languages) to `font`, for [`copy_with_descriptor`].
pub fn features_and_fallbacks_descriptor(
    font: &mut FontKitFont,
    features: &FontFeatures,
    fallbacks: Option<&FontFallbacks>,
) -> CTFontDescriptor {
    unsafe {
        let mut keys = vec![kCTFontFeatureSettingsAttribute];
        let mut values = vec![generate_feature_array(features)];
        if let Some(fallbacks) = fallbacks
            && !fallbacks.fallback_list().is_empty()
        {
            keys.push(kCTFontCascadeListAttribute);
            values.push(generate_fallback_array(fallbacks, font));
        }
        let attrs = CFDictionaryCreate(
            kCFAllocatorDefault,
            keys.as_ptr() as _,
            values.as_ptr() as _,
            keys.len() as isize,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        );

        for value in &values {
            CFRelease(*value as _);
        }

        let new_descriptor = CTFontDescriptorCreateWithAttributes(attrs);
        CFRelease(attrs as _);
        CTFontDescriptor::wrap_under_create_rule(new_descriptor)
    }
}

/// `CTFontCreateCopyWithAttributes`: `font` at `size` (0 keeps its size) with the
/// descriptor's attributes added.
pub fn copy_with_descriptor(font: &CTFont, size: f64, descriptor: &CTFontDescriptor) -> CTFont {
    unsafe {
        // font-kit uses the older core-text wrappers. Borrow the same CF objects
        // for the generated API, then transfer its create-rule ownership back.
        let generated_font = &*font
            .as_concrete_TypeRef()
            .cast::<objc2_core_text::CTFont>();
        let descriptor = &*descriptor
            .as_concrete_TypeRef()
            .cast::<objc2_core_text::CTFontDescriptor>();
        let new_font = generated_font.copy_with_attributes(size as _, std::ptr::null(), Some(descriptor));
        CTFont::wrap_under_create_rule(
            objc2_core_foundation::CFRetained::into_raw(new_font)
                .as_ptr()
                .cast(),
        )
    }
}

/// The system UI font the way Blink's `MatchSystemUIFont` makes it for `system-ui` and
/// `-apple-system`: `CTFontCreateUIFontForLanguage` at the exact size (so CoreText picks
/// the optical size and tracking for it), the bold trait from CSS weight 600 and the
/// italic trait when asked, then the CSS weight itself on the `wght` axis.
pub fn system_ui_font(size: f64, weight: f32, italic: bool) -> CTFont {
    use core_text::font_descriptor::{kCTFontBoldTrait, kCTFontVariationAttribute};
    let mut font = core_text::font::new_ui_font_for_language(
        core_text::font::kCTFontSystemFontType,
        size,
        None,
    );
    let mut traits = 0;
    if italic {
        traits |= kCTFontItalicTrait;
    }
    if weight >= 600. {
        traits |= kCTFontBoldTrait;
    }
    if traits != 0
        && let Some(styled) = font.clone_with_symbolic_traits(traits, traits)
    {
        font = styled;
    }
    if weight == 400. {
        return font;
    }
    const WGHT: i64 = 0x7767_6874;
    unsafe {
        let variations = core_foundation::dictionary::CFDictionary::from_CFType_pairs(&[(
            CFNumber::from(WGHT),
            CFNumber::from(weight as f64),
        )]);
        let attributes = core_foundation::dictionary::CFDictionary::from_CFType_pairs(&[(
            CFString::wrap_under_get_rule(kCTFontVariationAttribute),
            variations.as_CFType(),
        )]);
        let descriptor = core_text::font_descriptor::new_from_attributes(&attributes);
        copy_with_descriptor(&font, size, &descriptor)
    }
}

fn generate_feature_array(features: &FontFeatures) -> CFMutableArrayRef {
    unsafe {
        let feature_array = CFArrayCreateMutable(kCFAllocatorDefault, 0, &kCFTypeArrayCallBacks);
        for (tag, value) in features.tag_value_list() {
            let keys = [
                kCTFontOpenTypeFeatureTag as *const _,
                kCTFontOpenTypeFeatureValue as *const _,
            ];
            let tag = CFString::new(tag);
            let value = CFNumber::from(*value as i32);
            let values = [tag.as_CFTypeRef(), value.as_CFTypeRef()];
            let dict = CFDictionaryCreate(
                kCFAllocatorDefault,
                &keys as *const _ as _,
                &values as *const _ as _,
                2,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            CFArrayAppendValue(feature_array, dict as _);
            CFRelease(dict as _);
        }
        feature_array
    }
}

fn generate_fallback_array(fallbacks: &FontFallbacks, font: &mut FontKitFont) -> CFMutableArrayRef {
    unsafe {
        let symbolic_traits = font.native_font().symbolic_traits();
        let all_traits = font.native_font().all_traits();

        let fallback_array = CFArrayCreateMutable(kCFAllocatorDefault, 0, &kCFTypeArrayCallBacks);
        for user_fallback in fallbacks.fallback_list() {
            let name = CFString::from(user_fallback.as_str());

            let traits_keys = [kCTFontWeightTrait, kCTFontSlantTrait];
            let weight_value = CFNumber::from(all_traits.normalized_weight());
            let slant_value = CFNumber::from(if (symbolic_traits & kCTFontItalicTrait) != 0 {
                1.0
            } else {
                0.0
            });
            let traits_values = [weight_value.as_CFTypeRef(), slant_value.as_CFTypeRef()];
            let traits = CFDictionaryCreate(
                kCFAllocatorDefault,
                &traits_keys as *const _ as _,
                &traits_values as *const _ as _,
                traits_keys.len() as isize,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            drop(weight_value);
            drop(slant_value);

            let attr_keys = [kCTFontFamilyNameAttribute, kCTFontTraitsAttribute];
            let attr_values = [name.as_CFTypeRef(), traits as _];
            let attrs = CFDictionaryCreate(
                kCFAllocatorDefault,
                &attr_keys as *const _ as _,
                &attr_values as *const _ as _,
                attr_keys.len() as isize,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            CFRelease(traits as _);

            let fallback_desc = CTFontDescriptorCreateWithAttributes(attrs);
            CFRelease(attrs as _);

            CFArrayAppendValue(fallback_array, fallback_desc as _);
            CFRelease(fallback_desc as _);
        }

        append_system_fallbacks(fallback_array, &font.native_font());
        fallback_array
    }
}

fn append_system_fallbacks(fallback_array: CFMutableArrayRef, font: &CTFont) {
    unsafe {
        let preferred_languages: CFArray<CFString> =
            CFArray::wrap_under_create_rule(CFLocaleCopyPreferredLanguages());

        let default_fallbacks = cascade_list_for_languages(font, &preferred_languages);

        for desc in default_fallbacks
            .iter()
            .filter(|desc| desc.font_path().is_some())
        {
            CFArrayAppendValue(fallback_array, desc.as_concrete_TypeRef() as _);
        }
    }
}
