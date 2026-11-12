//! Column names of the DTCC public dissemination layout.
//!
//! The CSV slice and cumulative files use the CFTC technical-standard names
//! (`Notional amount-Leg 1`), and the dashboard's ticker JSON uses
//! camel-case keys (`notionalAmountLeg1`). Both normalize to the same key:
//! lower case with everything but ASCII letters and digits removed. A small
//! alias table covers the few names that differ beyond that.
//!
//! Columns the parser does not use are still *known* (listed in
//! [`KNOWN_UNUSED`]) so they are not reported as new. Anything else is an
//! unknown column: it is logged and counted, never fatal.

/// Normalizes a column name: ASCII letters and digits, lower case.
#[must_use]
pub fn normalize(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Columns the parser reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[allow(missing_docs)]
pub enum Field {
    DisseminationId,
    OriginalDisseminationId,
    ActionType,
    EventType,
    EventTimestamp,
    AmendmentIndicator,
    AssetClass,
    ProductName,
    ExecutionTimestamp,
    DisseminationTimestamp,
    EffectiveDate,
    ExpirationDate,
    BlockTradeElection,
    LargeNotionalOffFacilityElection,
    NotionalAmountLeg1,
    NotionalAmountLeg2,
    NotionalCurrencyLeg1,
    NotionalCurrencyLeg2,
    TotalNotionalQuantityLeg1,
    TotalNotionalQuantityLeg2,
    QuantityUnitOfMeasureLeg1,
    QuantityUnitOfMeasureLeg2,
    ExchangeRate,
    FirstExerciseDate,
    FixedRateLeg1,
    FixedRateLeg2,
    OptionPremiumAmount,
    OptionPremiumCurrency,
    Price,
    PriceNotation,
    PriceCurrency,
    SpreadLeg1,
    SpreadLeg2,
    SpreadNotationLeg1,
    SpreadNotationLeg2,
    StrikePrice,
    StrikePriceNotation,
    OptionType,
    OptionStyle,
    UnderlierIdLeg1,
    UnderlyingAssetName,
    Upi,
    UpiFisn,
    UpiUnderlierName,
}

impl Field {
    /// Every field the parser reads.
    pub const ALL: [Field; 44] = [
        Field::DisseminationId,
        Field::OriginalDisseminationId,
        Field::ActionType,
        Field::EventType,
        Field::EventTimestamp,
        Field::AmendmentIndicator,
        Field::AssetClass,
        Field::ProductName,
        Field::ExecutionTimestamp,
        Field::DisseminationTimestamp,
        Field::EffectiveDate,
        Field::ExpirationDate,
        Field::BlockTradeElection,
        Field::LargeNotionalOffFacilityElection,
        Field::NotionalAmountLeg1,
        Field::NotionalAmountLeg2,
        Field::NotionalCurrencyLeg1,
        Field::NotionalCurrencyLeg2,
        Field::TotalNotionalQuantityLeg1,
        Field::TotalNotionalQuantityLeg2,
        Field::QuantityUnitOfMeasureLeg1,
        Field::QuantityUnitOfMeasureLeg2,
        Field::ExchangeRate,
        Field::FirstExerciseDate,
        Field::FixedRateLeg1,
        Field::FixedRateLeg2,
        Field::OptionPremiumAmount,
        Field::OptionPremiumCurrency,
        Field::Price,
        Field::PriceNotation,
        Field::PriceCurrency,
        Field::SpreadLeg1,
        Field::SpreadLeg2,
        Field::SpreadNotationLeg1,
        Field::SpreadNotationLeg2,
        Field::StrikePrice,
        Field::StrikePriceNotation,
        Field::OptionType,
        Field::OptionStyle,
        Field::UnderlierIdLeg1,
        Field::UnderlyingAssetName,
        Field::Upi,
        Field::UpiFisn,
        Field::UpiUnderlierName,
    ];

    /// The column name in the CSV layout (PPD User Guide, Appendix A).
    #[must_use]
    pub const fn csv_name(self) -> &'static str {
        match self {
            Field::DisseminationId => "Dissemination Identifier",
            Field::OriginalDisseminationId => "Original Dissemination Identifier",
            Field::ActionType => "Action type",
            Field::EventType => "Event type",
            Field::EventTimestamp => "Event timestamp",
            Field::AmendmentIndicator => "Amendment indicator",
            Field::AssetClass => "Asset Class",
            Field::ProductName => "Product name",
            Field::ExecutionTimestamp => "Execution Timestamp",
            Field::DisseminationTimestamp => "Dissemination Timestamp",
            Field::EffectiveDate => "Effective Date",
            Field::ExpirationDate => "Expiration Date",
            Field::BlockTradeElection => "Block trade election indicator",
            Field::LargeNotionalOffFacilityElection => {
                "Large notional off-facility swap election indicator"
            }
            Field::NotionalAmountLeg1 => "Notional amount-Leg 1",
            Field::NotionalAmountLeg2 => "Notional amount-Leg 2",
            Field::NotionalCurrencyLeg1 => "Notional currency-Leg 1",
            Field::NotionalCurrencyLeg2 => "Notional currency-Leg 2",
            Field::TotalNotionalQuantityLeg1 => "Total notional quantity-Leg 1",
            Field::TotalNotionalQuantityLeg2 => "Total notional quantity-Leg 2",
            Field::QuantityUnitOfMeasureLeg1 => "Quantity unit of measure-Leg 1",
            Field::QuantityUnitOfMeasureLeg2 => "Quantity unit of measure-Leg 2",
            Field::ExchangeRate => "Exchange rate",
            Field::FirstExerciseDate => "First exercise date",
            Field::FixedRateLeg1 => "Fixed rate-Leg 1",
            Field::FixedRateLeg2 => "Fixed rate-Leg 2",
            Field::OptionPremiumAmount => "Option Premium Amount",
            Field::OptionPremiumCurrency => "Option Premium Currency",
            Field::Price => "Price",
            Field::PriceNotation => "Price notation",
            Field::PriceCurrency => "Price currency",
            Field::SpreadLeg1 => "Spread-Leg 1",
            Field::SpreadLeg2 => "Spread-Leg 2",
            Field::SpreadNotationLeg1 => "Spread notation-Leg 1",
            Field::SpreadNotationLeg2 => "Spread notation-Leg 2",
            Field::StrikePrice => "Strike Price",
            Field::StrikePriceNotation => "Strike price notation",
            Field::OptionType => "Option Type",
            Field::OptionStyle => "Option Style",
            Field::UnderlierIdLeg1 => "Underlier ID-Leg 1",
            Field::UnderlyingAssetName => "Underlying Asset Name",
            Field::Upi => "Unique Product Identifier",
            Field::UpiFisn => "UPI FISN",
            Field::UpiUnderlierName => "UPI Underlier Name",
        }
    }

    /// Extra normalized names (beyond `normalize(csv_name)`) accepted for
    /// the field, mostly the ticker JSON keys that differ.
    const fn aliases(self) -> &'static [&'static str] {
        match self {
            Field::DisseminationTimestamp => &["disseminationtime", "disseminationdatetime"],
            Field::Upi => &["upi"],
            Field::UpiFisn => &["uniqueproductidentifiershortname", "upishortname"],
            Field::UpiUnderlierName => &["uniqueproductidentifierunderliername"],
            Field::OptionPremiumAmount => &["optionpremium"],
            _ => &[],
        }
    }

    /// Looks a normalized column name up.
    #[must_use]
    pub fn from_normalized(n: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|f| normalize(f.csv_name()) == n || f.aliases().contains(&n))
    }
}

/// Columns of the published layout that the parser does not read. They are
/// recognized so that only genuinely new columns are reported as unknown.
/// Normalized form.
pub const KNOWN_UNUSED: &[&str] = &[
    "cleared",
    "mandatoryclearingindicator",
    "maturitydateoftheunderlier",
    "nonstandardizedtermindicator",
    "platformidentifier",
    "primebrokeragetransactionindicator",
    "notionalquantityleg1",
    "notionalquantityleg2",
    "quantityfrequencymultiplierleg1",
    "quantityfrequencymultiplierleg2",
    "quantityfrequencyleg1",
    "quantityfrequencyleg2",
    "notionalamountineffectonassociatedeffectivedateleg1",
    "notionalamountineffectonassociatedeffectivedateleg2",
    "effectivedateofthenotionalamountleg1",
    "effectivedateofthenotionalamountleg2",
    "enddateofthenotionalamountleg1",
    "enddateofthenotionalamountleg2",
    "callamount",
    "callamountleg1",
    "callamountleg2",
    "callcurrency",
    "callcurrencyleg1",
    "callcurrencyleg2",
    "putamount",
    "putamountleg1",
    "putamountleg2",
    "putcurrency",
    "putcurrencyleg1",
    "putcurrencyleg2",
    "exchangeratebasis",
    "priceunitofmeasure",
    "spreadcurrencyleg1",
    "spreadcurrencyleg2",
    "strikepricecurrencycurrencypair",
    "strikepricecurrencyorcurrencypair",
    "postpricedswapindicator",
    "fixedratedaycountconventionleg1",
    "fixedratedaycountconventionleg2",
    "floatingratedaycountconventionleg1",
    "floatingratedaycountconventionleg2",
    "floatingrateresetfrequencyperiodleg1",
    "floatingrateresetfrequencyperiodleg2",
    "floatingrateresetfrequencyperiodmultiplierleg1",
    "floatingrateresetfrequencyperiodmultiplierleg2",
    "otherpaymentamount",
    "fixedratepaymentfrequencyperiodleg1",
    "fixedratepaymentfrequencyperiodleg2",
    "floatingratepaymentfrequencyperiodleg1",
    "floatingratepaymentfrequencyperiodleg2",
    "fixedratepaymentfrequencyperiodmultiplierleg1",
    "fixedratepaymentfrequencyperiodmultiplierleg2",
    "floatingratepaymentfrequencyperiodmultiplierleg1",
    "floatingratepaymentfrequencyperiodmultiplierleg2",
    "otherpaymenttype",
    "otherpaymentcurrency",
    "settlementcurrencyleg1",
    "settlementcurrencyleg2",
    "settlementlocation",
    "settlementlocationleg1",
    "settlementlocationleg2",
    "collateralisationcategory",
    "custombasketindicator",
    "indexfactor",
    "underlieridleg2",
    "underlieridsourceleg1",
    "underlyingassetsubtypeorunderlyingcontractsubtypeleg1",
    "underlyingassetsubtypeorunderlyingcontractsubtypeleg2",
    "embeddedoptiontype",
    "packageindicator",
    "packagetransactionprice",
    "packagetransactionpricecurrency",
    "packagetransactionpricenotation",
    "packagetransactionspread",
    "packagetransactionspreadcurrency",
    "packagetransactionspreadnotation",
    "physicaldeliverylocationleg1",
    "deliverytype",
];

/// How a header column resolves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Column {
    /// A column the parser reads.
    Used(Field),
    /// A known column the parser ignores.
    KnownUnused,
    /// A column not in the documented layout (normalized name).
    Unknown(String),
}

/// Resolves one header cell.
#[must_use]
pub fn classify(header: &str) -> Column {
    let n = normalize(header.trim_start_matches('\u{feff}'));
    if let Some(f) = Field::from_normalized(&n) {
        Column::Used(f)
    } else if KNOWN_UNUSED.contains(&n.as_str()) {
        Column::KnownUnused
    } else {
        Column::Unknown(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_and_json_names_agree() {
        assert_eq!(
            classify("Notional amount-Leg 1"),
            Column::Used(Field::NotionalAmountLeg1)
        );
        assert_eq!(
            classify("notionalAmountLeg1"),
            Column::Used(Field::NotionalAmountLeg1)
        );
        assert_eq!(
            classify("largeNotionalOfffacilitySwapElectionIndicator"),
            Column::Used(Field::LargeNotionalOffFacilityElection)
        );
        assert_eq!(
            classify("blocktradeElectionIndicator"),
            Column::Used(Field::BlockTradeElection)
        );
        assert_eq!(
            classify("uniqueProductIdentifierShortName"),
            Column::Used(Field::UpiFisn)
        );
        assert_eq!(
            classify("\u{feff}Dissemination Identifier"),
            Column::Used(Field::DisseminationId)
        );
        assert_eq!(classify("EXCHANGERATEBASIS"), Column::KnownUnused);
        assert_eq!(
            classify("Strike price currency/currency pair"),
            Column::KnownUnused
        );
        assert_eq!(
            classify("Brand New Column"),
            Column::Unknown("brandnewcolumn".into())
        );
    }

    #[test]
    fn names_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for f in Field::ALL {
            assert!(seen.insert(normalize(f.csv_name())), "{f:?}");
            assert!(
                !KNOWN_UNUSED.contains(&normalize(f.csv_name()).as_str()),
                "{f:?}"
            );
        }
    }
}
