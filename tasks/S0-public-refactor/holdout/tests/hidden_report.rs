mod common;
use common::sample;

const REPORT: &str = "\
SKU        NAME                 QTY      VALUE\n\
A-100      Adjustable wrench      12     216.48\n\
B-220      Cordless drill with…    3     389.97\n\
C-305      Safety goggles        250    1060.37\n\
D-410      Hi-vis vest            60     675.00\n\
total safety                           1735.37\n\
total tools                             606.45\n\
grand total                            2341.82\n\
";

#[test]
fn report_text_is_unchanged() {
    assert_eq!(sample().render_report(), REPORT);
}
