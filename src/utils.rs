use std::io::{self, Read, BufRead, Write};
use std::fmt;
use std::format_args;
use std::process;

pub struct Console<T, U>
    where T: Read + BufRead, U: Write
{
    reader: T,
    writer: U,
    name:   String,
}

pub type ChildReader = io::BufReader<process::ChildStdout>;
pub type ChildWriter = process::ChildStdin;
pub type ConsoleFromChild = Console<ChildReader, ChildWriter>;

impl<T, U> Console<T, U>
    where T: Read + BufRead, U: Write
{
    // consumes the stdout and stdin of child,
    // no guarantee on the termination of child
    pub fn from_child(child: &mut process::Child, name: &str) -> ConsoleFromChild {
        Console {
            reader: io::BufReader::new(child.stdout.take().unwrap()),
            writer: child.stdin.take().unwrap(),
            name:   String::from(name),
        }
    }
}

pub struct ConsoleInteractor<T, U>
    where T: Read + BufRead, U: Write
{
    console:    Console<T, U>,
    delimeter:  u8,
}

impl<T, U> ConsoleInteractor<T, U>
where T: Read + BufRead, U: Write {
    pub fn new(console: Console<T, U>) -> ConsoleInteractor<T, U> {
        ConsoleInteractor {
            console,
            delimeter: b'>',
        }
    }

    pub fn send(&mut self, msg: &[u8]) -> io::Result<()> {
        self.console.writer.write_all(msg)?;
        self.console.writer.write_all(b"\n")
    }

    pub fn recv(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        let n = self.console.reader.read_until(self.delimeter, buf)?;
        buf.pop();
        Ok(n-1)
    }

    pub fn send_on_prompt(&mut self, msg: &[u8]) {
        let mut buf = Vec::new();
        self.recv(&mut buf).expect("Read prompt failed");
        self.log(format_args!("receive from console: {}", String::from_utf8(buf).unwrap()));
        self.log(format_args!("send to console: {}", String::from_utf8_lossy(msg)));
        self.send(msg).expect("Send message failed");
    }

    fn log(&self, args: fmt::Arguments) {
        print!("Console {}: ", self.console.name);
        println!("{}", args);
    }
}
