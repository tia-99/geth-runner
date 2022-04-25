use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::rc::{Rc, Weak};
use std::cell::RefCell;
use std::process::{self, Command, Stdio};

use crate::utils::{self, Console, ConsoleInteractor, ChildReader, ChildWriter, node_dir};
use crate::NETWORK_ID;

struct Node {
    peers:      Vec<Weak<RefCell<Node>>>,
    id:         usize,
    address:    String,
    itr:        Option<ConsoleInteractor<ChildReader, ChildWriter>>,
    enode:      Option<String>,
}

pub struct NodeRunner {
    geth_dir:       PathBuf,
    nodes_dir:      PathBuf,
    accounts_dir:   PathBuf,
    nodes:          Vec<Rc<RefCell<Node>>>,
    node_count:     usize,
    sealer_count:   usize,

    childs:         Vec<process::Child>,
}

impl NodeRunner {
    pub fn new_with_cfg_file(path: &Path) -> NodeRunner {
        let parsed = utils::read_toml(path);
        let mut nr = NodeRunner {
            geth_dir:       PathBuf::from_str(parsed["bin"]["geth_dir"].as_str().unwrap()).unwrap(),
            nodes_dir:      PathBuf::from_str(parsed["node"]["dir"].as_str().unwrap()).unwrap(),
            accounts_dir:   PathBuf::from_str(parsed["run"]["accounts_dir"].as_str().unwrap()).unwrap(),
            nodes:          Vec::new(),
            node_count:     parsed["node"]["count"].as_integer().unwrap() as usize,
            sealer_count:   parsed["node"]["sealer_count"].as_integer().unwrap() as usize,

            childs:          Vec::new(),
        };
        nr.nodes.reserve(nr.node_count);
        let addrs = utils::load_addrs(&nr.accounts_dir).unwrap();
        for (i, address) in addrs.into_iter().enumerate() {
            nr.nodes.push(Rc::new(RefCell::new(
                Node {
                    peers:      Vec::new(),
                    id:         i,
                    address,
                    itr:        None,
                    enode:      None,
                }
            )));
        }
        let conn = parsed["node"]["connection"].as_array().unwrap();
        for i in 0..nr.node_count {
            let peers = conn[i].as_array().unwrap();
            for p in peers {
                let pid = p.as_integer().unwrap() as usize;
                nr.nodes[i].borrow_mut().peers.push(
                    Rc::downgrade(&nr.nodes[pid])
                );
            }
        }

        nr
    }

    // consumes the value to avoid multiple calls on this function
    pub fn do_run_nodes(mut self) {
        for i in 0..self.nodes.len() {
            self.run_node(i);
        }
        self.connect_nodes();
        self.start_mining();
        loop {}
    }

    fn start_mining(&mut self) {
        for i in 0..self.sealer_count {
            let mut node = self.nodes[i].borrow_mut();
            node.itr.as_mut().unwrap().send_with_resp(b"miner.start()");
            node.itr.as_mut().unwrap().send_with_resp(b"clique.getSigners()");
            node.itr.as_mut().unwrap().send_with_resp(b"eth.accounts[0]");
            node.itr.as_mut().unwrap().send_with_resp(b"admin.peers");
        }
    }

    fn connect_nodes(&mut self) {
        for node in &mut self.nodes {
            let mut node = node.borrow_mut();
            for i in 0..node.peers.len() {
                let p = &node.peers[i];
                let prc = p.upgrade().unwrap();
                let pmut = prc.borrow();
                let enode = pmut.enode.as_ref().unwrap().clone();
                node.itr.as_mut().unwrap().send_with_resp(
                    format!("admin.addPeer(\"{}\")", enode).as_bytes()
                );
            }
        }
    }

    // runs the node and opens its console interactor
    fn run_node(&mut self, ith: usize) {
        let mut node = self.nodes[ith].borrow_mut();
        let mut geth = Command::new(&self.geth_dir)
            .arg(format!("--datadir={}", node_dir(&self.nodes_dir, node.id)))
            .arg(format!("--networkid={}", NETWORK_ID))
            .arg(format!("--port={}", 3000+node.id))
            .arg("console")
            .arg(format!("--ipcpath={}", Self::ipc_path(node.id)))
            .arg(format!("--unlock={}", node.address))
            .arg(format!("--password=password"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let console = Console::
            <utils::ChildReader, utils::ChildWriter>::
            from_child(&mut geth, &format!("node {}", node.id));
        match node.itr {
            None => node.itr = Some(ConsoleInteractor::new(console)),
            Some(_) => panic!("Initialized node console interactor"),
        }
        let mut buf = Vec::new();
        node.itr.as_mut().unwrap().recv(&mut buf).unwrap();
        let test_msg = b"eth.accounts[0]";
        let resp = node.itr.as_mut().unwrap().send_with_resp(test_msg);
        assert_eq!(resp[2..].to_uppercase(), node.address.clone().to_uppercase());

        match node.enode {
            None => {
                let enode = node.itr.as_mut().unwrap().send_with_resp(b"admin.nodeInfo.enode");
                node.enode = Some(enode);
            },
            Some(_) => panic!("Initialzied enode"),
        }

        self.childs.push(geth);
    }

    fn ipc_path(id: usize) -> String {
        format!("geth{}.ipc", id)
    }
}
