use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::rc::{Rc, Weak};
use std::cell::RefCell;
use std::process::{self, Command, Stdio};
use std::thread;
use std::time;
use rand::Rng;
use std::fs::{self, OpenOptions};

use crate::utils::{self, Console, ConsoleInteractor, ChildReader, ChildWriter, node_dir};
use crate::NETWORK_ID;

struct Node {
    peers:      Vec<Weak<RefCell<Node>>>,
    id:         usize,
    address:    String,
    itr:        Option<ConsoleInteractor<ChildReader, ChildWriter>>,
    enode:      Option<String>,
}

struct TestConfig {
    n:          usize,
    time_limit: time::Duration,
}

pub struct NodeRunner {
    geth_dir:       PathBuf,
    nodes_dir:      PathBuf,
    accounts_dir:   PathBuf,
    nodes:          Vec<Rc<RefCell<Node>>>,
    node_count:     usize,
    sealer_count:   usize,
    tr:             Option<TEERunner>,
    tf:             Option<TestConfig>,

    childs:         Vec<process::Child>,
}

impl NodeRunner {
    fn sample(k: i32, n: i32, cur: i32) -> Vec<i32> {
        if k > n {
            panic!("sample: k>n");
        }
        let mut rng = rand::thread_rng();
        let mut pool: Vec<i32> = (0..cur).chain(cur+1..n).collect();
        let k = k as usize;
        for i in k..pool.len() {
            let r = rng.gen_range(0..=i);
            if r < k {
                pool[r] = pool[i];
            }
        }
        pool[..k].to_vec()
    }

    pub fn new_with_cfg_file(path: &Path) -> NodeRunner {
        let parsed = utils::read_toml(path);
        let mut nr = NodeRunner {
            geth_dir:       PathBuf::from_str(parsed["bin"]["geth_dir"].as_str().unwrap()).unwrap(),
            nodes_dir:      PathBuf::from_str(parsed["node"]["dir"].as_str().unwrap()).unwrap(),
            accounts_dir:   PathBuf::from_str(parsed["run"]["accounts_dir"].as_str().unwrap()).unwrap(),
            nodes:          Vec::new(),
            node_count:     parsed["node"]["count"].as_integer().unwrap() as usize,
            sealer_count:   parsed["node"]["sealer_count"].as_integer().unwrap() as usize,
            tr:             None,
            tf:             None,

            childs:         Vec::new(),
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
        let mut random_conn = false;
        if let Some(rc) = parsed["node"].get("random_connect") {
            random_conn = rc.as_bool().unwrap();
        }
        if random_conn {
            let peer_count = parsed["node"]["peer_count"].as_integer().unwrap();
            for i in 0..nr.nodes.len() {
                let pids = Self::sample(peer_count as i32, nr.node_count as i32, i as i32);
                for pid in pids {
                    nr.nodes[i].borrow_mut().peers.push(
                        Rc::downgrade(&nr.nodes[pid as usize])
                    );
                }
            }
        } else {
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
        }
        if let Some(tee) = parsed["run"].get("tee") {
            let tee = tee.as_bool().unwrap();
            if tee {
                nr.tr = Some(TEERunner::new_with_cfg_file(path));
            }
        }

        if let Some(value) = parsed["test"].get("test") {
            let test = value.as_bool().unwrap();
            if test {
                nr.tf = Some(
                    TestConfig {
                        n:          parsed["test"]["n"].as_integer().unwrap() as usize,
                        time_limit: time::Duration::from_secs(parsed["test"]["period"].as_integer().unwrap() as u64),
                    }
                );
            }
        }

        nr
    }

    // consumes the value to avoid multiple calls on this function
    pub fn do_run_nodes(mut self) {
        if let Some(ref mut tr) = self.tr {
            tr.do_init_tee();
        }
        for i in 0..self.nodes.len() {
            // TODO: tee compatibility
            self.run_node(i);
        }
        self.connect_nodes();
        self.start_mining();
        let tf = self.tf.take();
        if let Some(tf) = tf {
            self.test_send_txs(tf.n, tf.time_limit);
        } else {
            loop {}
        }
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
        // let output = OpenOptions::new()
        //                 .write(true)
        //                 .create(true)
        //                 .open(format!("node{}.txt", ith))
        //                 .unwrap();
        let mut geth = Command::new(&self.geth_dir)
            .arg(format!("--datadir={}", node_dir(&self.nodes_dir, node.id)))
            .arg(format!("--networkid={}", NETWORK_ID))
            .arg(format!("--port={}", 3000+node.id))
            .arg("console")
            .arg(format!("--ipcpath={}", Self::ipc_path(node.id)))
            .arg(format!("--unlock={}", node.address))
            .arg(format!("--password=password"))
            // .arg(format!("2> out{}.txt", ith))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // .stderr(Stdio::piped())
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

    fn test_send_txs(&mut self, n: usize, time_limit: time::Duration) {
        let before = self.get_tx_cnt();
        println!("Transaction counts before sending tx: {:?}", before);
        let ddl = time::Instant::now() + time_limit;
        self.send_txs(n, ddl);
        thread::sleep(ddl - time::Instant::now());
        let after = self.get_tx_cnt();
        println!("Transaction counts after sending tx: {:?}", after);
        let dif: Vec<usize> = (0..self.nodes.len()).map(|i| after[i]-before[i]).collect();
        println!("Transaction committed for each node: {:?}", dif);
        println!("Total committed transactions: {}", dif.into_iter().sum::<usize>());
    }

    fn send_txs(&mut self, n: usize, ddl: time::Instant) {
        for i in 0..n {
            if time::Instant::now() >= ddl {
                break;
            }
            for j in 0..self.nodes.len() {
                self.send_tx(j, (j+1)%self.nodes.len(), i)
            }
        }
    }

    fn get_tx_cnt(&mut self) -> Vec<usize> {
        let mut res = Vec::with_capacity(self.nodes.len());
        for node in &mut self.nodes {
            let mut node = node.borrow_mut();
            let itr = node.itr.as_mut().unwrap();
            let cnt = itr.send_with_resp(b"eth.getTransactionCount(eth.accounts[0])");
            let cnt: usize = str::parse(&cnt).unwrap();
            res.push(cnt);
        }
        res
    }

    fn send_tx(&mut self, x: usize, y: usize, nonce: usize) {
        let msg = format!(
            "eth.sendTransaction({{from:\"{}\", to:\"{}\", nonce: \"{}\", value:web3.toWei(1e+45, \"ether\")}})",
            self.nodes[x].borrow().address,
            self.nodes[y].borrow().address,
            nonce,
        );
        let msg = msg.as_bytes();
        self.nodes[x].borrow_mut().itr.as_mut().unwrap().send_with_resp(msg);
    }

    fn ipc_path(id: usize) -> String {
        format!("geth{}.ipc", id)
    }
}

pub struct TEERunner {
    _node_count:     usize,
    ip:             String,
    username:       String,
    _opensgx_dir:    PathBuf,
}

impl TEERunner {
    pub fn new_with_cfg_file(path: &Path) -> TEERunner {
        let parsed = utils::read_toml(path);
        TEERunner {
            _node_count:     parsed["node"]["count"].as_integer().unwrap() as usize,
            ip:             String::from(parsed["remote"]["ip"].as_str().unwrap()),
            username:       String::from(parsed["remote"]["username"].as_str().unwrap()),
            _opensgx_dir:    PathBuf::from(parsed["remote"]["opensgx_dir"].as_str().unwrap()),
        }
    }

    pub fn do_init_tee(&self) {
        let mut _remote = Command::new("ssh")
            .arg("-T")
            .arg(format!("{}@{}", self.username, self.ip))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_sample() {
        const N: i32 = 100000;
        let mut cnt = [0; 5];
        for _ in 0..N {
            let res = super::NodeRunner::sample(3, 5, 0);
            // println!("{:?}", res);
            for x in res {
                cnt[x as usize] += 1;
            }
        }
        println!("{:?}", cnt);
    }

    #[test]
    fn test_sample2() {
        const N: i32 = 100;
        for _ in 0..N {
            let res = super::NodeRunner::sample(6, 24, 14);
            println!("{:?}", res);
        }
    }
}