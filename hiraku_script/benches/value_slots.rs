//! CPU/cache microbenchmark only; not a claim about whole-game throughput.
use hiraku_script::{Register, RegisterFrame, Value};
use std::{hint::black_box, time::Instant};

#[inline(always)]
fn enum_add(
    values: &mut [Value],
    dst: Register,
    left: Register,
    right: Register,
) -> Result<(), hiraku_script::VmError> {
    use hiraku_script::VmError;
    let a = values
        .get(left.0 as usize)
        .ok_or(VmError::InvalidRegister(left))?;
    let b = values
        .get(right.0 as usize)
        .ok_or(VmError::InvalidRegister(right))?;
    let (Value::Int(a), Value::Int(b)) = (a, b) else {
        return Err(VmError::TypeMismatch("Int"));
    };
    let value = a.checked_add(*b).ok_or(VmError::IntegerOverflow)?;
    *values
        .get_mut(dst.0 as usize)
        .ok_or(VmError::InvalidRegister(dst))? = Value::Int(value);
    Ok(())
}

fn main() {
    const COUNT: u16 = 60_000;
    const PASSES: usize = 100;
    let mut packed = RegisterFrame::new(COUNT + 1);
    packed
        .write(Register(COUNT), Value::Int(1))
        .expect("increment");
    let mut enums = vec![Value::Int(0); COUNT as usize + 1];
    enums[COUNT as usize] = Value::Int(1);
    for i in 0..COUNT {
        packed
            .write(Register(i), Value::Int(0))
            .expect("valid register");
    }
    let start = Instant::now();
    for _ in 0..PASSES {
        for i in 0..COUNT {
            enum_add(
                black_box(&mut enums),
                Register(i),
                Register(i),
                Register(COUNT),
            )
            .expect("integer addition");
        }
    }
    let enum_time = start.elapsed();
    let start = Instant::now();
    for _ in 0..PASSES {
        for i in 0..COUNT {
            black_box(&mut packed)
                .binary(
                    Register(i),
                    hiraku_script::BinaryOp::Add,
                    Register(i),
                    Register(COUNT),
                )
                .expect("integer addition");
        }
    }
    let packed_time = start.elapsed();
    assert_eq!(
        packed.read(Register(COUNT - 1)),
        Some(Value::Int(PASSES as i64))
    );
    println!(
        "enum value: {} bytes; packed slot: 8 bytes",
        size_of::<Value>()
    );
    println!(
        "{} updates: enum {:?}, packed {:?}",
        usize::from(COUNT) * PASSES,
        enum_time,
        packed_time
    );
    black_box((enums, packed));
}
