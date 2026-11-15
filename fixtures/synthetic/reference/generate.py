"""Generates the synthetic reference-source fixtures. Every value is invented."""
import datetime as dt
import json
import os
import sys

root = sys.argv[1]
out = os.path.join(root, 'fixtures/synthetic/reference')


def w(rel, text):
    p = os.path.join(out, rel)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, 'w', newline='') as f:
        f.write(text)


def bdays(start, end):
    d = start
    while d <= end:
        if d.weekday() < 5:
            yield d
        d += dt.timedelta(days=1)


# --- NY Fed -----------------------------------------------------------
def nyfed_rate(kind, d, base):
    i = (d - dt.date(2025, 12, 1)).days
    r = round(base + ((i * 7) % 5 - 2) * 0.01, 2)
    rec = {
        "effectiveDate": d.isoformat(),
        "type": kind,
        "percentRate": r,
        "percentPercentile1": round(r - 0.05, 2),
        "percentPercentile25": round(r - 0.01, 2),
        "percentPercentile75": round(r + 0.01, 2),
        "percentPercentile99": round(r + 0.08, 2),
        "volumeInBillions": 2000 + i,
        "revisionIndicator": "",
    }
    if kind == "EFFR":
        rec["targetRateFrom"] = 4.25
        rec["targetRateTo"] = 4.50
    return rec


days = list(bdays(dt.date(2025, 12, 1), dt.date(2026, 3, 16)))
secured = [nyfed_rate(k, d, b) for d in reversed(days) for k, b in (("SOFR", 4.31), ("BGCR", 4.29), ("TGCR", 4.28))]
# An undocumented key on one record: reported, never emitted.
secured[0]["exampleUndocumentedKey"] = "x"
w('nyfed/secured_all.json', json.dumps({"refRates": secured}, indent=1) + "\n")
rev = json.loads(json.dumps(secured))
for r in rev:
    if r["type"] == "SOFR" and r["effectiveDate"] == "2026-03-16":
        r["percentRate"] = round(r["percentRate"] + 0.03, 2)
        r["revisionIndicator"] = "Y"
w('nyfed/secured_all_revised.json', json.dumps({"refRates": rev}, indent=1) + "\n")
effr = [nyfed_rate("EFFR", d, 4.33) for d in reversed(days)]
w('nyfed/effr.json', json.dumps({"refRates": effr}, indent=1) + "\n")
obfr = [nyfed_rate("OBFR", d, 4.32) for d in reversed(days)]
# A malformed record: skipped and counted.
obfr.append({"effectiveDate": "not-a-date", "type": "OBFR", "percentRate": 4.3})
w('nyfed/obfr.json', json.dumps({"refRates": obfr}, indent=1) + "\n")
sofrai = []
idx = 1.20000000
for d in days:
    idx = round(idx * (1 + 0.0431 / 360), 8)
    sofrai.append({"effectiveDate": d.isoformat(), "type": "SOFRAI", "average30day": 4.30123, "average90day": 4.29876,
                   "average180day": 4.35001, "index": idx, "revisionIndicator": ""})
w('nyfed/sofrai.json', json.dumps({"refRates": list(reversed(sofrai))}, indent=1) + "\n")

# --- Treasury rate feeds ------------------------------------------------
def atom(entries):
    s = ['<?xml version="1.0" encoding="utf-8" standalone="yes"?>',
         '<feed xml:base="https://example.invalid/" xmlns="http://www.w3.org/2005/Atom" '
         'xmlns:d="http://schemas.microsoft.com/ado/2007/08/dataservices" '
         'xmlns:m="http://schemas.microsoft.com/ado/2007/08/dataservices/metadata">',
         '  <title type="text">Synthetic</title>']
    for i, props in enumerate(entries):
        s.append('  <entry>')
        s.append(f'    <id>synthetic-{i}</id>')
        s.append('    <content type="application/xml">')
        s.append('      <m:properties>')
        s.append(f'        <d:Id m:type="Edm.Int32">{i + 1}</d:Id>')
        for k, v in props:
            if v is None:
                s.append(f'        <d:{k} m:null="true" />')
            else:
                typ = 'Edm.DateTime' if 'DATE' in k else ('Edm.String' if k.startswith(('CUSIP', 'RATE_TYPE')) else 'Edm.Double')
                s.append(f'        <d:{k} m:type="{typ}">{v}</d:{k}>')
        s.append('      </m:properties>')
        s.append('    </content>')
        s.append('  </entry>')
    s.append('</feed>')
    return "\n".join(s) + "\n"


tenors = [("BC_1MONTH", 4.20), ("BC_1_5MONTH", 4.21), ("BC_2MONTH", 4.22), ("BC_3MONTH", 4.24), ("BC_4MONTH", 4.25),
          ("BC_6MONTH", 4.22), ("BC_1YEAR", 4.10), ("BC_2YEAR", 3.95), ("BC_3YEAR", 3.90), ("BC_5YEAR", 3.95),
          ("BC_7YEAR", 4.05), ("BC_10YEAR", 4.20), ("BC_20YEAR", 4.55), ("BC_30YEAR", 4.60)]
tdays = list(bdays(dt.date(2026, 3, 2), dt.date(2026, 3, 16)))
entries = []
for j, d in enumerate(tdays):
    props = [("NEW_DATE", d.isoformat() + "T00:00:00")]
    for k, v in tenors:
        props.append((k, f"{v + 0.01 * (j % 3):.2f}"))
    props.append(("BC_30YEARDISPLAY", f"{4.60 + 0.01 * (j % 3):.2f}"))
    if j == len(tdays) - 1:
        # A tenor Treasury might add: read and reported.
        props.append(("BC_9MONTH", "4.15"))
    if j == 0:
        # An absent value on one date (element missing means no value).
        props = [p for p in props if p[0] != "BC_1_5MONTH"]
    entries.append(props)
w('treasury/daily_treasury_yield_curve_2026.xml', atom(entries))

real = []
for j, d in enumerate(tdays):
    real.append([("NEW_DATE", d.isoformat() + "T00:00:00")] + [(k, f"{v:.2f}") for k, v in
                (("TC_5YEAR", 1.60), ("TC_7YEAR", 1.75), ("TC_10YEAR", 1.90), ("TC_20YEAR", 2.20), ("TC_30YEAR", 2.30))])
w('treasury/daily_treasury_real_yield_curve_2026.xml', atom(real))

bills = []
weeks = [(4, 4.18), (6, 4.19), (8, 4.20), (13, 4.21), (17, 4.20), (26, 4.15), (52, 4.00)]
for j, d in enumerate(tdays):
    props = [("INDEX_DATE", d.isoformat() + "T00:00:00"), ("QUOTE_DATE", d.isoformat() + "T00:00:00")]
    for n, r in weeks:
        mat = d + dt.timedelta(days=7 * n)
        disc = r + 0.01 * (j % 2)
        props += [
            (f"ROUND_B1_CLOSE_{n}WK_2", f"{disc:.2f}"),
            (f"ROUND_B1_YIELD_{n}WK_2", f"{disc + 0.08:.2f}"),
            (f"CUSIP_{n}WK", f"SYNTH{n:04d}"),
            (f"MATURITY_DATE_{n}WK", mat.isoformat() + "T00:00:00"),
            (f"CS_{n}WK_CLOSE_AVG", f"{disc:.3f}"),
            (f"CS_{n}WK_YIELD_AVG", f"{disc + 0.08:.3f}"),
        ]
    bills.append(props)
# A malformed value: skipped and counted.
bills[0] = [(k, ("n/a?" if k == "ROUND_B1_CLOSE_4WK_2" else v)) for k, v in bills[0]]
w('treasury/daily_treasury_bill_rates_2026.xml', atom(bills))

lt = []
for d in tdays:
    lt.append([("QUOTE_DATE", d.isoformat() + "T00:00:00"), ("RATE_TYPE", "BC_20year"), ("RATE", "4.56"), ("EXTRAPOLATION_FACTOR", None)])
    lt.append([("QUOTE_DATE", d.isoformat() + "T00:00:00"), ("RATE_TYPE", "Over_10_Years"), ("RATE", "4.40"), ("EXTRAPOLATION_FACTOR", None)])
w('treasury/daily_treasury_long_term_rate_2026.xml', atom(lt))

# --- Fiscal Data auctions -----------------------------------------------
def auction(cusip, kind, term, adate, idate, mdate, **kv):
    row = {"record_date": adate, "cusip": cusip, "security_type": kind, "security_term": term,
           "auction_date": adate, "issue_date": idate, "maturity_date": mdate,
           "offering_amt": "80000000000", "total_tendered": "230000000000", "total_accepted": "80000000000",
           "bid_to_cover_ratio": "2.88", "primary_dealer_accepted": "20000000000", "soma_accepted": "0",
           "ref_cpi_on_issue_date": "null", "frn_index_determination_rate": "null",
           "pdf_filenm_announcemt": "A_SYNTHETIC.pdf", "xml_filenm_comp_results": "R_SYNTHETIC.xml"}
    for k in ("high_yield", "low_yield", "avg_med_yield", "high_discnt_rate", "low_discnt_rate", "avg_med_discnt_rate",
              "high_investment_rate", "low_investment_rate", "avg_med_investment_rate", "high_price", "low_price",
              "avg_med_price", "int_rate"):
        row[k] = kv.get(k, "null")
    return row


p1 = [
    auction("SYNTH0001", "Bill", "13-Week", "2026-03-09", "2026-03-12", "2026-06-11",
            high_discnt_rate="4.205", high_investment_rate="4.301", high_price="98.937014", low_discnt_rate="4.150", avg_med_discnt_rate="4.185"),
    auction("SYNTH0002", "Note", "10-Year", "2026-03-10", "2026-03-16", "2036-02-15",
            high_yield="4.215", low_yield="4.100", avg_med_yield="4.180", high_price="99.880000", int_rate="4.125"),
]
tips = auction("SYNTH0003", "Note", "10-Year", "2026-03-11", "2026-03-16", "2036-01-15",
               high_yield="1.905", high_price="99.500000", int_rate="1.875")
tips["ref_cpi_on_issue_date"] = "321.12345"
p2 = [tips, auction("SYNTH0004", "Bill", "52-Week", "2026-03-12", "2026-03-17", "2027-03-16",
                    high_discnt_rate="3.950", high_investment_rate="4.120", high_price="96.006111")]
p2.append({"auction_date": "garbage"})
meta = {"count": 2, "labels": {"cusip": "CUSIP"}, "total-count": 5, "total-pages": 2}
w('fiscaldata/auctions_page1.json', json.dumps({"data": p1, "meta": meta,
                                                 "links": {"self": "&page%5Bnumber%5D=1", "next": "&page%5Bnumber%5D=2"}}, indent=1) + "\n")
w('fiscaldata/auctions_page2.json', json.dumps({"data": p2, "meta": meta,
                                                 "links": {"self": "&page%5Bnumber%5D=2", "next": None}}, indent=1) + "\n")

# --- Board H.15 and H.10 (SDMX-ML 1.0 style) -----------------------------
def sdmx(series):
    s = ['<?xml version="1.0" encoding="UTF-8"?>',
         '<message:MessageGroup xmlns:message="http://www.SDMX.org/resources/SDMXML/schemas/v1_0/message" '
         'xmlns:frb="http://www.federalreserve.gov/structure/compact/common" '
         'xmlns:kf="http://www.federalreserve.gov/structure/compact/H15_H15">',
         '<message:Header><message:ID>SYNTHETIC</message:ID></message:Header>',
         '<frb:DataSet id="SYNTHETIC">']
    for name, attrs, obs in series:
        a = " ".join(f'{k}="{v}"' for k, v in attrs.items())
        s.append(f'<kf:Series SERIES_NAME="{name}" {a}>')
        s.append('<frb:Annotations><common:Annotation xmlns:common="x"><common:AnnotationType>Short Description</common:AnnotationType>'
                 '<common:AnnotationText>Synthetic</common:AnnotationText></common:Annotation></frb:Annotations>')
        for p, v in obs:
            # As published: no data is OBS_STATUS="ND" with the placeholder -9999.
            s.append(f'<frb:Obs OBS_STATUS="{"ND" if v == "ND" else "A"}" OBS_VALUE="{"-9999" if v == "ND" else v}" TIME_PERIOD="{p}" />')
        s.append('</kf:Series>')
    s.append('</frb:DataSet></message:MessageGroup>')
    return "\n".join(s) + "\n"


h15days = [d.isoformat() for d in bdays(dt.date(2026, 3, 9), dt.date(2026, 3, 13))]
def obs(base, nd_on=None):
    return [(p, "ND" if p == nd_on else f"{base + 0.01 * i:.2f}") for i, p in enumerate(h15days)]


h15 = [
    ("RIFSPFF_N.B", {"FREQ": "9", "UNIT": "Percent"}, obs(4.33)),
    ("RIFSPPNAAD30_N.B", {"FREQ": "9"}, obs(4.30)),
    ("RIFSRP_F02_N.B", {"FREQ": "9"}, obs(4.50)),
    ("RIFSGFSW04_N.B", {"FREQ": "9"}, obs(4.18)),
    ("RIFLGFCY10_N.B", {"FREQ": "9"}, obs(4.20, nd_on=h15days[2])),
    ("RIFLGFCY05_XII_N.B", {"FREQ": "9"}, obs(1.60)),
    ("RIFLGFCY10_N.WF", {"FREQ": "17"}, [("2026-03-13", "4.22")]),
    ("RIFSPBLP_N.B", {"FREQ": "9"}, obs(7.50)),
    # Excluded by ADM-0008: vendor, benchmark administrator, undocumented.
    ("RIFLDIY10_N.B", {"FREQ": "9"}, obs(3.90)),
    ("RILSPDEPM03_N.B", {"FREQ": "9"}, obs(4.40)),
    ("RIMLPAAAR_N.B", {"FREQ": "9"}, obs(5.20)),
    ("H0.RIFSPPCM01_N.B", {"FREQ": "9"}, obs(5.00)),
    # Not on the allowlist: reported, not emitted.
    ("RIFNEWSERIES_N.B", {"FREQ": "9"}, obs(1.00)),
]
w('frb/H15_data.xml', sdmx(h15))
h15_rev = [(n, a, [(p, ("4.35" if (n == "RIFSPFF_N.B" and p == h15days[-1]) else v)) for p, v in o]) for n, a, o in h15]
w('frb/H15_data_revised.xml', sdmx(h15_rev))
h10 = [
    ("RXI$US_N.B.EU", {"FREQ": "9", "CURRENCY": "USD", "FX": "EUR"}, obs(1.08)),
    ("RXI_N.B.JA", {"FREQ": "9", "CURRENCY": "JPY", "FX": "USD"}, obs(149.50)),
    ("RXI_N.B.CA", {"FREQ": "9", "CURRENCY": "CAD", "FX": "USD"}, obs(1.35)),
    # Excluded by ADM-0009: dollar indexes.
    ("JRXWTFB_N.B", {"FREQ": "9"}, obs(120.00)),
    # Unknown currency code: reported, not emitted.
    ("RXI_N.B.ZZ", {"FREQ": "9"}, obs(2.00)),
]
w('frb/H10_data.xml', sdmx(h10))

# --- CFTC COT -------------------------------------------------------------
legacy_header = ["Market and Exchange Names", "As of Date in Form YYMMDD", "As of Date in Form YYYY-MM-DD",
                 "CFTC Contract Market Code", "CFTC Market Code in Initials", "CFTC Region Code", "CFTC Commodity Code",
                 "Open Interest (All)", "Noncommercial Positions-Long (All)", "Noncommercial Positions-Short (All)",
                 "Noncommercial Positions-Spreading (All)", "Commercial Positions-Long (All)",
                 "Commercial Positions-Short (All)", " Total Reportable Positions-Long (All)",
                 "Total Reportable Positions-Short (All)", "Nonreportable Positions-Long (All)",
                 "Nonreportable Positions-Short (All)", "Open Interest (Old)", "Change in Open Interest (All)",
                 "% of OI-Noncommercial-Long (All)", "Traders-Total (All)", "Concentration-Gross LT = 4 TDR-Long (All)",
                 "Contract Units"]


def legacy_row(name, code, d, oi, base):
    nl, ns, sp = base, base // 2, base // 10
    cl, cs = oi - nl - sp - 500, oi - ns - sp - 400
    tl, ts = nl + sp + cl, ns + sp + cs
    return [name, d.strftime("%y%m%d"), d.isoformat(), code, "SYN", "00", "000", str(oi), str(nl), str(ns), str(sp),
            str(cl), str(cs), str(tl), str(ts), str(oi - tl), str(oi - ts), str(oi), "120", "33.3", "55", "40.1",
            "(CONTRACTS OF 1 UNIT)"]


def csv_line(cells):
    return ",".join('"' + c + '"' if not c.replace(".", "").isdigit() else c for c in cells)


rows = []
for d in (dt.date(2025, 12, 16), dt.date(2025, 12, 23), dt.date(2025, 12, 30)):
    rows.append(legacy_row("SYNTHETIC COMMODITY A - EXAMPLE MARKET", "000001", d, 10000, 3000))
    rows.append(legacy_row("SYNTHETIC COMMODITY B - EXAMPLE MARKET", "00000B", d, 5000, 1000))
bad = legacy_row("SYNTHETIC COMMODITY C - EXAMPLE MARKET", "000003", dt.date(2025, 12, 30), 1000, 100)
bad[8] = "12x"  # malformed count: skipped
rows.append(bad)
w('cot/legacy_fut_2025.txt', "\n".join([csv_line(legacy_header)] + [csv_line(r) for r in rows]) + "\n")
rows26 = [legacy_row("SYNTHETIC COMMODITY A - EXAMPLE MARKET", "000001", dt.date(2026, 1, 6), 10200, 3100)]
w('cot/legacy_fut_2026.txt', "\n".join([csv_line(legacy_header)] + [csv_line(r) for r in rows26]) + "\n")
w('cot/legacy_com_2026.txt', "\n".join([csv_line(legacy_header)] + [csv_line(legacy_row("SYNTHETIC COMMODITY A - EXAMPLE MARKET", "000001", dt.date(2026, 1, 6), 12500, 3500))]) + "\n")
weekly = [legacy_row("SYNTHETIC COMMODITY A - EXAMPLE MARKET", "000001", dt.date(2026, 1, 13), 10300, 3150)]
w('cot/deafut_weekly.txt', "\n".join(csv_line(r) for r in weekly) + "\n")

dis_header = ["Market_and_Exchange_Names", "As_of_Date_In_Form_YYMMDD", "Report_Date_as_YYYY-MM-DD",
              "CFTC_Contract_Market_Code", "CFTC_Market_Code", "Open_Interest_All", "Prod_Merc_Positions_Long_All",
              "Prod_Merc_Positions_Short_All", "Swap_Positions_Long_All", "Swap__Positions_Short_All",
              "Swap__Positions_Spread_All", "M_Money_Positions_Long_All", "M_Money_Positions_Short_All",
              "M_Money_Positions_Spread_All", "Other_Rept_Positions_Long_All", "Other_Rept_Positions_Short_All",
              "Other_Rept_Positions_Spread_All", "Tot_Rept_Positions_Long_All", "Tot_Rept_Positions_Short_All",
              "NonRept_Positions_Long_All", "NonRept_Positions_Short_All", "Traders_Tot_All"]
dis = [["SYNTHETIC COMMODITY A - EXAMPLE MARKET", "260106", "2026-01-06", "000001", "SYN", "20000", "4000", "6000",
        "3000", "2500", "500", "5000", "3000", "800", "1500", "1200", "300", "15100", "14300", "4900", "5700", "90"]]
w('cot/disagg_com_2026.txt', "\n".join([csv_line(dis_header)] + [csv_line(r) for r in dis]) + "\n")

cit_header = ["Market_and_Exchange_Names", "As_of_Date_In_Form_YYMMDD", "Report_Date_as_YYYY-MM-DD",
              "CFTC_Contract_Market_Code", "Open_Interest_All", "NComm_Positions_Long_All_NoCIT",
              "NComm_Positions_Short_All_NoCIT", "NComm_Postions_Spread_All_NoCIT", "Comm_Positions_Long_All_NoCIT",
              "Comm_Positions_Short_All_NoCIT", "Tot_Rept_Positions_Long_All", "Tot_Rept_Positions_Short_All",
              "NonRept_Positions_Long_All", "NonRept_Positions_Short_All", "CIT_Positions_Long_All", "CIT_Positions_Short_All"]
cit = [["SYNTHETIC COMMODITY A - EXAMPLE MARKET", "260106", "2026-01-06", "000001", "12500", "3000", "1500", "400",
        "5000", "8000", "11000", "10500", "1500", "2000", "2600", "600"]]
w('cot/cit_2026.txt', "\n".join([csv_line(cit_header)] + [csv_line(r) for r in cit]) + "\n")

# --- USDA LMR -------------------------------------------------------------
def lmr_row(pt, base, date="03/10/2026", **extra):
    r = {"report_date": date, "for_date_begin": "03/09/2026", "published_date": date + " 08:30:00",
         "purchase_type": pt, "head_count": "5,123", "base_price": f"{base:.2f}", "avg_net_price": f"{base + 2:.2f}",
         "lowest_net_price": f"{base - 5:.2f}", "highest_net_price": f"{base + 6:.2f}", "avg_live_weight": "285.10",
         "avg_carcass_weight": "214.20", "avg_sort_loss": "-2.10", "avg_backfat": "0.62", "avg_loin_depth": "2.65",
         "loineye_area": "8.10", "avg_lean_percent": "55.9", "is_correction": "N"}
    r.update(extra)
    return r


swine = [
    {"reportSection": "Barrows/Gilts", "reportSections": ["Barrows/Gilts", "Carcass Measurements"], "stats": {"totalRows": 9},
     "results": [
         lmr_row("Prod. Sold Negotiated", 90.10),
         lmr_row("Prod. Sold Negotiated Formula", 91.20),
         lmr_row("Prod. Sold Swine or Pork Market Formula", 92.30),
         lmr_row("Prod. Sold Combined", 90.50),
         lmr_row("Prod. Sold Other Market Formula", 99.90),
         lmr_row("Prod. Sold Other Purchase Arrangement", 98.80),
         lmr_row("Prod. Sold (All Purchase Types)", 97.70),
         lmr_row("Pack. Sold (All Purchase Types)", 96.60),
         lmr_row("Packer Owned", 95.50),
         lmr_row("Prod. Sold Example New Type", 94.40),
     ]},
    {"reportSection": "Carcass Measurements", "results": [{"report_date": "03/10/2026", "wtd_avg_base": "93.00", "wtd_avg_net_price": "94.00"}]},
]
w('lmr/2511.json', json.dumps(swine, indent=1) + "\n")
swine_corr = json.loads(json.dumps(swine))
swine_corr[0]["results"][0]["base_price"] = "90.35"
swine_corr[0]["results"][0]["is_correction"] = "Y"
w('lmr/2511_corrected.json', json.dumps(swine_corr, indent=1) + "\n")
cattle = [
    {"reportSection": "Summary", "results": [{"report_date": "03/10/2026", "current_volume": "12,000"}]},
    {"reportSection": "Detail", "results": [
        {"report_date": "03/10/2026", "class_description": "STEER", "selling_basis_description": "LIVE FOB",
         "grade_description": "Over 80% Choice", "head_count": "1,234", "weight_range_low": "1350", "weight_range_high": "1600",
         "weight_range_avg": "1475", "price_range_low": "238.00", "price_range_high": "240.00", "weighted_avg_price": "239.12"},
        {"report_date": "03/10/2026", "class_description": "HEIFER", "selling_basis_description": "DRESSED DELIVERED",
         "grade_description": "65 - 80% Choice", "head_count": "456", "weight_range_low": "800", "weight_range_high": "1000",
         "weight_range_avg": "905", "price_range_low": "378.00", "price_range_high": "380.00", "weighted_avg_price": "379.40"},
    ]},
]
w('lmr/2466.json', json.dumps(cattle, indent=1) + "\n")

# --- USDA dairy -------------------------------------------------------------
def nd(section, week, fields):
    return {"reportSection": section, "results": [dict({"week_ending_date": week, "report_date": week}, **fields)]}


ndpsr = [
    nd("Butter Prices and Sales", "03/07/2026", {"Butter_Price": "2.5512", "Butter_Sales": "4,123,456"}),
    nd("40 Pound Block Cheddar Cheese Prices and Sales", "03/07/2026", {"cheese_40_Price": "1.8123", "cheese_40_Sales": "10,234,567"}),
    nd("500 Pound Barrel Cheddar Cheese Prices, Sales, and Moisture Content", "03/07/2026",
       {"cheese_500_Price": "1.7801", "cheese_500_Wtd_Price": "1.8001", "cheese_500_Sales": "9,876,543", "cheese_500_Wtd_Mst": "34.85"}),
    nd("Dry Whey Prices and Sales", "03/07/2026", {"whey_Price": "0.5512", "whey_Sales": "6,543,210"}),
    nd("Nonfat Dry Milk Prices and Sales", "03/07/2026",
       {"nonfat_milk_Price": "1.2012", "nonfat_milk_Sales": "15,432,109", "CME_Example_Spot_Price": "1.2500"}),
    nd("Final Butter Prices and Sales", "02/28/2026", {"Butter_Price": "2.5401", "Butter_Sales": "4,000,001"}),
]
ndpsr_first = [nd("Butter Prices and Sales", "02/28/2026", {"Butter_Price": "2.5399", "Butter_Sales": "4,000,000"})]
w('dairy/2993_week1.json', json.dumps(ndpsr_first, indent=1) + "\n")
w('dairy/2993.json', json.dumps(ndpsr, indent=1) + "\n")
cls = [{"reportSection": "Detail", "results": [{
    "report_year": "2026", "report_month": "February", "class_2_Price": "18.12", "class_2_butterfat_Price": "2.6023",
    "advanced_skim_milk_class_2_Price": "9.01", "class_3_Price": "17.25", "class_3_skim_milk_Price": "8.80",
    "class_4_Price": "16.90", "class_4_skim_milk_Price": "8.20", "butterfat_Price": "2.5953", "nonfat_solids_Price": "1.0011",
    "protein_Price": "2.3456", "other_solids_Price": "0.3210", "somatic_cell_adjustment_Rate": "0.00091",
    "butter_monthly_avg_Price": "2.5500", "nfdm_monthly_avg_Price": "1.2000", "cheese_monthly_avg_Price": "1.8000",
    "cheese_monthly_40p_Price": "1.8100", "cheese_monthly_500p_Price": "1.7900", "whey_monthly_avg_Price": "0.5500",
    "example_undocumented_Price": "9.99", "cme_futures_settle": "17.00"}]}]
w('dairy/2991.json', json.dumps(cls, indent=1) + "\n")

# --- USDA Market News feeder cattle (fixed-layout text) ---------------------
feeder = """SJ_LS850
Synthetic City, ST    Fri Mar 06, 2026    USDA-Example Market News

                 National Feeder & Stocker Cattle Summary
                         Week Ending: 03/06/2026

  Receipts:   This Week      Last Week     Last Year
             250,000        240,000       230,000

  Compared to last week, steer calves traded firm. Futures moves were
  quoted 2.00 higher on the week; no futures prices are carried here.
  Quotes 500-600 lbs 245.00-262.50 in this narrative are not a table row.

Feeder Steers Medium and Large 1
Head   Wt Range   Avg Wt    Price Range     Avg Price
 1250   500-600     552    245.00-262.50     253.84
  900   600-700     651    230.00-244.00     237.12
   80   600-700     648    250.00            250.00   Fleshy
  120   700-800     755    220.00-226.00     223.50   Basis priced
   60   800-900     850    210.00-215.00     212.40   Futures plus
Feeder Heifers Medium and Large 1-2
Head   Wt Range   Avg Wt    Price Range     Avg Price
  700   400-500     455    250.00-270.00     260.10
   40   500-600     5x0    240.00-250.00     245.00

Source:  Synthetic fixture; values invented.
"""
w('feeder/sj_ls850.txt', feeder)
w('feeder/sj_ls850_stale.txt', feeder.replace("03/06/2026", "01/02/2026").replace("Mar 06, 2026", "Jan 02, 2026"))

# --- ERCOT ------------------------------------------------------------------
def listing(rid, docs):
    s = ['<html><body><table class="reportTable">', '<tr><th>Name</th><th>Size</th><th>Download</th></tr>']
    for doc, name in docs:
        s.append(f'<tr><td class="labelOptional_ind">{name}</td><td>12K</td>'
                 f'<td><div><a href="/misdownload/servlets/mirDownload?mimic_duns=000000000&doclookupId={doc}">zip</a></div></td></tr>')
    s.append('</table></body></html>')
    return "\n".join(s) + "\n"


w('ercot/listing_12331.html', listing(12331, [
    (900001, "cdr.00012331.0000000000000000.20260309.131500.DAMSPNP4190_csv.zip"),
    (900002, "cdr.00012331.0000000000000000.20260309.131500.DAMSPNP4190_xml.zip"),
]))
dam = ["DeliveryDate,HourEnding,SettlementPoint,SettlementPointPrice,DSTFlag"]
for h in range(1, 25):
    dam.append(f"03/10/2026,{h:02d}:00,HB_NORTH,{25 + h * 0.5:.2f},N")
    dam.append(f"03/10/2026,{h:02d}:00,LZ_HOUSTON,{26 + h * 0.5:.2f},N")
dam.append("03/10/2026,25:00,HB_NORTH,1.00,N")  # malformed hour: skipped
w('ercot/dam_spp.csv', "\n".join(dam) + "\n")
dam_corr = [l.replace("HB_NORTH,30.00", "HB_NORTH,31.25") for l in dam]
w('ercot/dam_spp_corrected.csv', "\n".join(dam_corr) + "\n")
fall = ["DeliveryDate,HourEnding,SettlementPoint,SettlementPointPrice,DSTFlag",
        "11/01/2026,01:00,HB_NORTH,20.00,N", "11/01/2026,02:00,HB_NORTH,21.00,N", "11/01/2026,02:00,HB_NORTH,22.00,Y",
        "11/01/2026,03:00,HB_NORTH,-5.50,N"]
w('ercot/dam_spp_fallback.csv', "\n".join(fall) + "\n")
rt = ["DeliveryDate,DeliveryHour,DeliveryInterval,SettlementPointName,SettlementPointType,SettlementPointPrice,DSTFlag,ExampleNewColumn"]
for i in range(1, 5):
    rt.append(f"03/10/2026,14,{i},HB_NORTH,HU,{30 + i:.2f},N,x")
    rt.append(f"03/10/2026,14,{i},LZ_WEST,LZ,{31 + i:.2f},N,x")
w('ercot/rt_spp.csv', "\n".join(rt) + "\n")
w('ercot/listing_13060.html', listing(13060, [
    (800025, "rpt.00013060.0000000000000000.DAMLZHBSPP_2025.zip"),
    (800026, "rpt.00013060.0000000000000000.DAMLZHBSPP_2026.zip"),
]))
# --- EIA bulk files (ADM-0021) and BLS time-series files (ADM-0022) ---
EIA_SRC = "EIA, U.S. Energy Information Administration"


def series(sid, units, f, data, copyright="None", source=EIA_SRC, updated="2026-11-10T09:52:01-05:00", **extra):
    o = {"series_id": sid, "name": "Synthetic series " + sid, "units": units, "f": f,
         "unitsshort": "x", "description": "Synthetic fixture series; every value is invented.",
         "copyright": copyright, "source": source, "iso3166": "USA", "geography": "USA",
         "start": data[-1][0] if data else "", "end": data[0][0] if data else "",
         "last_updated": updated, "data": data}
    o.update(extra)
    return json.dumps(o, separators=(',', ':'))


pet = [
    json.dumps({"category_id": "900001", "parent_category_id": "900000", "name": "Synthetic petroleum",
                "notes": "", "childseries": ["PET.EMM_EPMR_PTE_NUS_DPG.W"]}, separators=(',', ':')),
    # Weekly retail regular gasoline: two weeks, a gap and an old value.
    series("PET.EMM_EPMR_PTE_NUS_DPG.W", "Dollars per Gallon", "W",
           [["20261109", 3.111], ["20261102", 3.125], ["20261026", None], ["19950102", 1.111]]),
    # Weekly retail diesel: a quoted value, an NA marker and a malformed value.
    series("PET.EMD_EPD2D_PTE_NUS_DPG.W", "Dollars per Gallon", "W",
           [["20261109", "3.950"], ["20261102", "NA"], ["20261026", "abc"]]),
    # Residential heating oil (admitted) and the wholesale series beside it (OPIS, excluded).
    series("PET.W_EPD2F_PRS_NUS_DPG.W", "Dollars per Gallon", "W", [["20261109", 4.012], ["20261102", 3.998]],
           updated="2026-11-11T13:05:00-05:00"),
    series("PET.W_EPD2F_PWR_NUS_DPG.W", "Dollars per Gallon", "W", [["20261109", 2.501]]),
    # Spot and futures prices labelled as EIA's own (the mislabel the admission found): excluded.
    series("PET.RWTC.D", "Dollars per Barrel", "D", [["20261109", 61.01]]),
    series("PET.EER_EPMRU_PF4_Y35NY_DPG.D", "Dollars per Gallon", "D", [["20261109", 2.01]],
           copyright="Thomson-Reuters", source="Thomson Reuters"),
    series("PET.RCLC1.D", "Dollars per Barrel", "D", [["20240405", 60.01]]),
    # Admitted volumes: not emitted.
    series("PET.WCESTUS1.W", "Thousand Barrels", "W", [["20261106", 400001], ["20261030", 400002]]),
    series("PET.W_EPC0_SAX_YCUOK_MBBL.W", "Thousand Barrels", "W", [["20261106", 20001]]),
    # Monthly refiner sales price and crude first purchase price: exchange_referenced.
    series("PET.EMA_EPMR_PTR_NUS_DPG.M", "Dollars per Gallon", "M", [["202609", 2.801], ["202608", 2.790]],
           updated="2026-11-02T10:00:00-05:00"),
    series("PET.F000000__3.M", "Dollars per Barrel", "M", [["202608", 61.25]]),
    # An allowlisted series that now credits a vendor: dropped and reported.
    series("PET.EMM_EPMRR_PTE_NUS_DPG.W", "Dollars per Gallon", "W", [["20261109", 3.301]],
           copyright="Thomson-Reuters"),
    # An allowlisted price series in unexpected units: dropped and reported.
    series("PET.EMM_EPM0_PTE_NUS_DPG.W", "Cents per Gallon", "W", [["20261109", 311.1]]),
    # An EIA series that is not on the allowlist.
    series("PET.MCRIMUS2.M", "Thousand Barrels", "M", [["202608", 6001]]),
]
w('eia/PET.txt', '\n'.join(pet) + '\n')

ng = [
    json.dumps({"category_id": "900101", "parent_category_id": "900100", "name": "Synthetic natural gas",
                "notes": "", "childseries": []}, separators=(',', ':')),
    series("NG.NW2_EPG0_SWO_R48_BCF.W", "Billion Cubic Feet", "W", [["20261106", 3801]]),
    series("NG.N3050US3.M", "Dollars per Thousand Cubic Feet", "M", [["202608", 4.10], ["202607", 4.05]]),
    series("NG.N3010US3.M", "Dollars per Thousand Cubic Feet", "M", [["202608", "W"], ["202607", 15.01]]),
    # Henry Hub spot, labelled as EIA's own: excluded.
    series("NG.RNGWHHD.D", "Dollars per Million Btu", "D", [["20261109", 3.01]]),
    # Natural gas import prices are not among the admitted monthly prices.
    series("NG.N9100US3.M", "Dollars per Thousand Cubic Feet", "M", [["202608", 3.51]]),
    # A petroleum series in the natural gas file: excluded.
    series("PET.EMM_EPMR_PTE_NUS_DPG.W", "Dollars per Gallon", "W", [["20261109", 3.111]]),
]
w('eia/NG.txt', '\n'.join(ng) + '\n')


def bls(rows):
    head = "series_id        \tyear\tperiod\t       value\tfootnote_codes\r\n"
    body = ''.join(f"{sid:<17}\t{y}\t{p}\t{v:>12}\t{fn}\r\n" for sid, y, p, v, fn in rows)
    return head + body


w('bls/cu.data.0.Current', bls([
    ("CUSR0000SA0", "2026", "M08", "321.500", ""),
    ("CUSR0000SA0", "2026", "M09", "322.250", ""),
    ("CUUR0000SA0", "1996", "M12", "158.600", ""),
    ("CUUR0000SA0", "2025", "M10", "-", "X"),
    ("CUUR0000SA0", "2025", "M13", "318.000", ""),
    ("CUUR0000SA0", "2026", "M08", "322.012", ""),
    ("CUUR0000SA0", "2026", "M09", "322.456", ""),
    ("CUUR0000SA0E", "2026", "M14", "300.000", ""),
    ("CUUR0000SA0L1E", "2026", "M09", "330.100", ""),
    ("CUURS49ASA0", "2026", "M09", "350.000", ""),
    ("CUUS0000SA0", "2026", "S01", "321.000", ""),
    ("CUUR0000SAF1", "20X6", "M09", "330.000", ""),
    ("CUXR0000SA0", "2026", "M09", "322.000", ""),
    ("WPU00000000", "2026", "M09", "250.000", ""),
]))
# The February revision of seasonally adjusted values.
w('bls/cu.data.0.Current.revised', bls([
    ("CUSR0000SA0", "2026", "M08", "321.600", "R"),
    ("CUSR0000SA0", "2026", "M09", "322.250", ""),
    ("CUUR0000SA0", "2026", "M08", "322.012", ""),
    ("CUUR0000SA0", "2026", "M09", "322.456", ""),
]))
w('bls/cu.data.1.AllItems', bls([
    ("CUUR0000AA0", "1990", "M01", "381.500", ""),
    ("CUUR0000SA0", "1990", "M01", "127.400", ""),
    ("CUURS49ASA0", "1990", "M01", "130.100", ""),
]))
w('bls/cw.data.0.Current', bls([
    ("CWUR0000SA0", "2026", "M09", "315.200", ""),
    ("CWSR0000SA0", "2026", "M09", "314.900", ""),
]))
w('bls/su.data.0.Current', bls([
    ("SUUR0000SA0", "2025", "M09", "168.100", ""),
    ("SUUR0000SA0", "2026", "M08", "172.345", "I"),
    ("SUUR0000SA0", "2026", "M09", "172.880", "I"),
]))
# Interim values replace initial ones.
w('bls/su.data.0.Current.revised', bls([
    ("SUUR0000SA0", "2025", "M09", "168.100", ""),
    ("SUUR0000SA0", "2026", "M08", "172.401", "U"),
    ("SUUR0000SA0", "2026", "M09", "172.880", "I"),
]))
w('bls/ap.data.0.Current', bls([
    ("APU0000709112", "2026", "M09", "4.123", ""),
    ("APU000074714", "2026", "M09", "3.210", ""),
    ("APUS49A74714", "2026", "M09", "3.900", ""),
    ("APU0000709112", "2026", "M10", "", "X"),
]))

print("fixtures written")
