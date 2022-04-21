use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::io::prelude::*;
use std::str::FromStr;
use std::error::Error;
use toml::Value;
use std::fmt;
use std::process::{Command, Stdio};
use std::env;
use std::{thread, time};

use crate::utils::{self, Console, ConsoleInteractor};

const NETWORK: &str = "auto_test";

struct InitError;

impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl fmt::Debug for InitError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl Error for InitError {}

type Address = String;

#[derive(Debug)]
pub struct NodeInitializer {
    geth_dir:       PathBuf,
    puppeth_dir:    PathBuf,
    data_dir:       PathBuf,
    node_count:     usize,
    sealer_count:   usize,
    out:            PathBuf,
}

impl NodeInitializer {
    pub fn new_with_cfg_file(path: &Path) -> NodeInitializer {
        let mut file = File::open(&path).unwrap();
        let mut contents = String::new();
        file.read_to_string(&mut contents).unwrap();
        let parsed = contents.parse::<Value>().unwrap();
        NodeInitializer {
            geth_dir:       PathBuf::from_str(parsed["bin"]["geth_dir"].as_str().unwrap()).unwrap(),
            puppeth_dir:    PathBuf::from_str(parsed["bin"]["puppeth_dir"].as_str().unwrap()).unwrap(),
            data_dir:       PathBuf::from_str(parsed["init"]["nodes_dir"].as_str().unwrap()).unwrap(),
            node_count:     parsed["init"]["node_count"].as_integer().unwrap() as usize,
            sealer_count:   parsed["init"]["sealer_count"].as_integer().unwrap() as usize,
            out:            PathBuf::from_str(parsed["init"]["accounts_dir"].as_str().unwrap()).unwrap(),
        }
    }

    pub fn do_init_node(&self) {
        let accounts = self.create_accounts();
        self.create_genesis(&accounts);
        self.init_nodes();
    }

    fn init_nodes(&self) {
        let mut genesis_dir = self.data_dir.clone();
        genesis_dir.push(Path::new(&format!("{}.json", NETWORK)));
        let genesis_dir = genesis_dir.into_os_string().into_string().unwrap();

        for i in 0..self.node_count {
            self.init_node(i, &genesis_dir);
        }
    }

    fn init_node(&self, id: usize, genesis_dir: &str) {
        let mut geth = Command::new(&self.geth_dir)
            .arg(format!("--datadir={}", self.data_dir(id)))
            .arg("init")
            .arg(genesis_dir)
            .spawn()
            .unwrap();
        geth.wait().unwrap();
    }

    // assumes self.node_count >= self.sealer_count
    fn create_gesis(&self, accounts: &Vec<Address>) {
        let mut dir = env::current_dir().unwrap();
        dir.push(Path::new(".puppeth"));
        let exist = dir.is_dir();

        let mut puppeth = Command::new(&self.puppeth_dir)
            .stdin(Stdio::piped())
            // .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut pin = puppeth.stdin.take().unwrap();

        pin.write_all(NETWORK.as_bytes()).unwrap();
        pin.write_all(b"\n").unwrap();
        if exist {
            println!("Target network exisits, rewriting...");
            pin.write_all(b"2\n3\n").unwrap();
        }
        pin.write_all(b"2\n1\n2\n\n").unwrap();

        for i in 0..self.sealer_count {
            pin.write_all(accounts[i].as_bytes()).unwrap();
            pin.write_all(b"\n").unwrap();
        }
        pin.write_all(b"\n").unwrap();

        for i in 0..self.node_count {
            pin.write_all(accounts[i].as_bytes()).unwrap();
            pin.write_all(b"\n").unwrap();
        }
        pin.write_all(b"\n").unwrap();

        pin.write_all(b"\n").unwrap();
        pin.write_all(b"\n").unwrap();

        pin.write_all(b"2\n2\n").unwrap();
        let genesis_path = self.data_dir.clone().into_os_string().into_string().unwrap();
        pin.write_all(genesis_path.as_bytes()).unwrap();
        pin.write_all(b"\n").unwrap();

        // dirty hack: wait the child process to finish writing
        // TODO: find a better way to do it
        thread::sleep(time::Duration::from_secs(1));

        puppeth.kill().unwrap();
    }

    // assumes self.node_count >= self.sealer_count
    fn create_genesis(&self, accounts: &Vec<Address>) {
        let mut dir = env::current_dir().unwrap();
        dir.push(Path::new(".puppeth"));
        let exist = dir.is_dir();

        let mut puppeth = Command::new(&self.puppeth_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        
        let console = Console
            ::<utils::ChildReader, utils::ChildWriter>
            ::from_child(&mut puppeth, "PUPPETH");
        
        let mut itr = ConsoleInteractor::new(console);
        itr.send_on_prompt(NETWORK.as_bytes());

        if exist {
            println!("Target network exisits, rewriting...");
            itr.send_on_prompt(b"2");
            itr.send_on_prompt(b"3");
        }
        itr.send_on_prompt(b"2");
        itr.send_on_prompt(b"1");
        itr.send_on_prompt(b"2");
        itr.send_on_prompt(b"");

        for i in 0..self.sealer_count {
            itr.send_on_prompt(accounts[i].as_bytes());
        }
        itr.send_on_prompt(b"");

        for i in 0..self.node_count {
            itr.send_on_prompt(accounts[i].as_bytes());
        }
        itr.send_on_prompt(b"");

        itr.send_on_prompt(b"");
        itr.send_on_prompt(b"");

        itr.send_on_prompt(b"2");
        itr.send_on_prompt(b"2");

        let genesis_path = self.data_dir.clone().into_os_string().into_string().unwrap();
        itr.send_on_prompt(genesis_path.as_bytes());

        itr.send_on_prompt(b"");

        puppeth.kill().unwrap();
    }

    fn create_accounts(&self) -> Vec<Address> {
        let mut accounts = vec![];
        for i in 0..self.node_count {
            let account = self.create_account(i);
            accounts.push(account);
        }
        let accounts_des = toml::to_string(&accounts).unwrap();
        let mut accounts_file = OpenOptions::new()
            .write(true)
            .create(true)
            .open(&self.out)
            .unwrap();
        accounts_file.write_all(accounts_des.as_bytes()).unwrap();
        accounts
    }

    fn create_account(&self, id: usize) -> Address {
        let mut geth = Command::new(&self.geth_dir)
            .arg(format!("--datadir={}", self.data_dir(id)))
            .arg("account")
            .arg("new")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut geth_in = geth.stdin.take().unwrap();
        let mut geth_out = geth.stdout.take().unwrap();
        geth_in.write_all(b"\n\n").unwrap();
        let mut res = String::new();
        geth_out.read_to_string(&mut res).unwrap();
        let idx = res.find("0x").unwrap() + 2;
        res[idx..(idx+40)].to_string()
    }

    fn data_dir(&self, id: usize) -> String {
        let mut data_dir = self.data_dir.clone();
        let mut subdir = format!("node{}/data", id);
        data_dir.push(Path::new(&mut subdir));
        data_dir.into_os_string().into_string().unwrap()
    }
}