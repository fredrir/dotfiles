//! A small Prettier 3.9 conformance corpus.
//!
//! Expected files were produced with `prettier --no-config --parser typescript`
//! (3.9.10). Tailwind sorting is disabled here so the comparison is about
//! layout, not class order.

use std::path::Path;

use dotfmt_typescript::{Config, Dialect, format};

fn formatted(input: &str, dialect: Dialect) -> String {
    let config = Config {
        tailwind: dotfmt_tailwind::Config {
            mode: dotfmt_tailwind::Mode::Off,
            ..Default::default()
        },
        ..Config::default()
    };
    format(Path::new("fixture.tsx"), input, &config, dialect).expect("format")
}

#[test]
fn typescript_layout_matches_prettier() {
    let input = r#"interface User {name:string;age?:number}
const  value = {alpha:1,beta:[1,2,3]} as const
export function greet(user:User):string{return `Hello ${user.name}`}
"#;
    let expected = r#"interface User {
  name: string;
  age?: number;
}
const value = { alpha: 1, beta: [1, 2, 3] } as const;
export function greet(user: User): string {
  return `Hello ${user.name}`;
}
"#;
    assert_eq!(formatted(input, Dialect::Ts), expected);
}

#[test]
fn tsx_layout_matches_prettier() {
    let input = r#"import {useState} from "react";
export function Counter({initial=0}:{initial?:number}) {
  const [count,setCount]=useState(initial);
  return <button className="p-4 flex items-center gap-2" onClick={()=>setCount((value)=>value+1)}>{count}</button>
}
"#;
    let expected = r#"import { useState } from "react";
export function Counter({ initial = 0 }: { initial?: number }) {
  const [count, setCount] = useState(initial);
  return (
    <button
      className="p-4 flex items-center gap-2"
      onClick={() => setCount((value) => value + 1)}
    >
      {count}
    </button>
  );
}
"#;
    assert_eq!(formatted(input, Dialect::Tsx), expected);
}

#[test]
fn decorators_and_type_members_match_prettier_style() {
    let input = r#"@sealed class Point {constructor(public x:number,public y:number){}}
type Result<T> = {ok:true,value:T}|{ok:false,error:string}
"#;
    let expected = r#"@sealed
class Point {
  constructor(
    public x: number,
    public y: number,
  ) {}
}
type Result<T> = { ok: true; value: T } | { ok: false; error: string };
"#;
    assert_eq!(formatted(input, Dialect::Ts), expected);
}
