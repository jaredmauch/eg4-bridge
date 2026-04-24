mod common;

use common::Factory;
use eg4_bridge::eg4::packet::ReadInputs;

#[test]
fn read_inputs_default() {
    let read_inputs = ReadInputs::default();
    assert_eq!(read_inputs.to_input_all(), None);
}

#[test]
fn read_inputs_set() {
    let mut read_inputs = ReadInputs::default();
    read_inputs.set_read_input_1(Factory::read_input_1());
    assert_eq!(read_inputs.to_input_all(), None);
}

#[tokio::test]
#[cfg_attr(not(feature = "mocks"), ignore)]
async fn handles_missing_read_input() {
    let r1 = Factory::read_input_1();
    let r2 = Factory::read_input_2();
    let r3 = Factory::read_input_3();
    let r4 = Factory::read_input_4();
    let r5 = Factory::read_input_5();
    let r6 = Factory::read_input_6();

    let mut read_inputs = ReadInputs::default();
    read_inputs.set_read_input_1(r1.clone());
    assert_eq!(read_inputs.to_input_all(), None);

    read_inputs.set_read_input_2(r2.clone());
    assert_eq!(read_inputs.to_input_all(), None);

    read_inputs.set_read_input_3(r3.clone());
    assert_eq!(read_inputs.to_input_all(), None);

    read_inputs.set_read_input_4(r4.clone());
    read_inputs.set_read_input_5(r5.clone());
    read_inputs.set_read_input_6(r6.clone());

    let expected = {
        let mut ri = ReadInputs::default();
        ri.set_read_input_1(r1);
        ri.set_read_input_2(r2);
        ri.set_read_input_3(r3);
        ri.set_read_input_4(r4);
        ri.set_read_input_5(r5);
        ri.set_read_input_6(r6);
        ri.to_input_all()
    };
    assert_eq!(read_inputs.to_input_all(), expected);

    let mut read_inputs = ReadInputs::default();
    read_inputs.set_read_input_3(Factory::read_input_3());
    assert_eq!(read_inputs.to_input_all(), None);
}
