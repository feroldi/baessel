#![feature(hash_raw_entry)]

use std::{
    cell::{Cell, Ref, RefCell},
    collections::{hash_map::RawEntryMut, HashMap},
    fmt,
    hash::{BuildHasher, Hash, Hasher},
    ptr,
};

/// An index for a NodeData in a NodeCtxt.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct NodeId(usize);

/// An index for a RegionData in a NodeCtxt.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct RegionId(usize);

/// An index for a UserData of an input or result port.
#[derive(Clone, Copy, PartialEq, Debug)]
enum UserId {
    In { node: NodeId, index: usize },
    Res { region: RegionId, index: usize },
}

impl UserId {
    fn node(&self) -> Option<NodeId> {
        match self {
            &UserId::In { node, .. } => Some(node),
            _ => None,
        }
    }
}

/// An index for an OriginData of an output or argument port.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum OriginId {
    Out { node: NodeId, index: usize },
    Arg { region: RegionId, index: usize },
}

impl OriginId {
    fn node(&self) -> Option<NodeId> {
        match self {
            &OriginId::Out { node, .. } => Some(node),
            _ => None,
        }
    }
}

/// A UserData contains information about an input or result port.
struct UserData {
    origin: OriginId,
    sink: Option<OriginId>,
    prev_user: Cell<Option<UserId>>,
    next_user: Cell<Option<UserId>>,
}

/// An OriginData contains information about an output or argument port.
#[derive(Clone, Default, Debug)]
struct OriginData {
    source: Option<UserId>,
    users: Cell<Option<UserIdList>>,
}

/// A linked list of users connected to a common origin.
#[derive(Clone, Copy, PartialEq, Debug)]
struct UserIdList {
    first: UserId,
    last: UserId,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum NodeKind<S> {
    Op(S),
    Apply {
        arg_val_ins: usize,
        arg_st_ins: usize,
        region_val_res: usize,
        region_st_res: usize,
    },
    Gamma {
        val_ins: usize,
        val_outs: usize,
        st_ins: usize,
        st_outs: usize,
    },
}

struct NodeData<S> {
    ins: Vec<UserData>,
    outs: Vec<OriginData>,
    inner_regions: Option<InnerRegionList>,
    outer_region: RegionId,
    kind: NodeKind<S>,
}

struct InnerRegionList {
    first_region: RegionId,
    last_region: RegionId,
}

struct RegionData {
    res: Vec<UserData>,
    args: Vec<OriginData>,
    prev_region: Cell<Option<RegionId>>,
    next_region: Cell<Option<RegionId>>,
}

#[derive(Debug, Copy, Clone, PartialEq, Default)]
struct SigS {
    val_ins: usize,
    val_outs: usize,
    st_ins: usize,
    st_outs: usize,
}

impl SigS {
    fn ins_len(&self) -> usize {
        self.val_ins + self.st_ins
    }

    fn outs_len(&self) -> usize {
        self.val_outs + self.st_outs
    }
}

trait Sig {
    fn sig(&self) -> SigS;
}

impl<S: Sig> Sig for NodeData<S> {
    fn sig(&self) -> SigS {
        self.kind.sig()
    }
}

impl<S: Sig> Sig for NodeKind<S> {
    fn sig(&self) -> SigS {
        match self {
            NodeKind::Op(s) => s.sig(),
            &NodeKind::Apply {
                arg_val_ins,
                arg_st_ins,
                region_val_res,
                region_st_res,
            } => SigS {
                val_ins: 1 + arg_val_ins, // function input + argument inputs
                st_ins: arg_st_ins,
                val_outs: region_val_res,
                st_outs: region_st_res,
            },
            &NodeKind::Gamma {
                val_ins,
                val_outs,
                st_ins,
                st_outs,
            } => {
                SigS {
                    val_ins: 1 + val_ins, // predicate + inputs
                    val_outs,
                    st_ins,
                    st_outs,
                }
            }
        }
    }
}

struct NodeTerm<S> {
    kind: NodeKind<S>,
    origins: Vec<OriginId>,
}

impl<S: PartialEq> PartialEq for NodeTerm<S> {
    fn eq(&self, other: &NodeTerm<S>) -> bool {
        self.kind == other.kind && self.origins == other.origins
    }
}

impl<S: Eq> Eq for NodeTerm<S> {}

impl<S: Hash> Hash for NodeTerm<S> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.kind.hash(state);
        self.origins.hash(state);
    }
}

struct NodeCtxt<S> {
    nodes: RefCell<Vec<NodeData<S>>>,
    regions: RefCell<Vec<RegionData>>,
    interned_nodes: RefCell<HashMap<NodeTerm<S>, NodeId>>,
}

impl<S> NodeCtxt<S> {
    fn new() -> NodeCtxt<S>
    where
        S: Eq + Hash,
    {
        NodeCtxt {
            nodes: RefCell::new(vec![]),
            regions: RefCell::new(vec![]),
            interned_nodes: RefCell::default(),
        }
    }

    fn node_data(&self, id: NodeId) -> Ref<NodeData<S>> {
        Ref::map(self.nodes.borrow(), |nodes| &nodes[id.0])
    }

    fn region_data(&self, id: RegionId) -> Ref<RegionData> {
        Ref::map(self.regions.borrow(), |regions| &regions[id.0])
    }

    fn user_data(&self, user_id: UserId) -> Ref<UserData> {
        match user_id {
            UserId::In { node, index } => {
                Ref::map(self.node_data(node), |node_data| &node_data.ins[index])
            }
            UserId::Res { region, index } => Ref::map(self.region_data(region), |region_data| {
                &region_data.res[index]
            }),
        }
    }

    fn origin_data(&self, origin_id: OriginId) -> Ref<OriginData> {
        match origin_id {
            OriginId::Out { node, index } => {
                Ref::map(self.node_data(node), |node_data| &node_data.outs[index])
            }
            OriginId::Arg { region, index } => Ref::map(self.region_data(region), |region_data| {
                &region_data.args[index]
            }),
        }
    }

    fn hash_node_terms(&self, node_kind: &NodeKind<S>, origins: &[OriginId]) -> u64
    where
        S: Eq + Hash,
    {
        let mut hasher = self.interned_nodes.borrow().hasher().build_hasher();
        node_kind.hash(&mut hasher);
        origins.hash(&mut hasher);
        hasher.finish()
    }

    fn mk_node_with(&self, kind: NodeKind<S>, origins: &[OriginId]) -> NodeId
    where
        S: Sig + Eq + Hash + Clone,
    {
        assert_eq!(kind.sig().ins_len(), origins.len());

        let create_node = |kind: NodeKind<S>, origins: &[OriginId]| {
            // Node creation works as follows:
            // 1. Create the UserData sequence, operanding sinks as you go.
            // 2. Initialize the OutData sequence with empty users.
            // 3. Push the new node to the node context and return its id.

            // Input ports are put into this vector so the node creation comes down to just
            // a push into the `self.nodes`.
            let mut new_node_inputs = Vec::<UserData>::with_capacity(kind.sig().ins_len());
            let node_id = NodeId(self.nodes.borrow().len());

            for (i, &origin) in origins.iter().enumerate() {
                let new_in_id = UserId::In {
                    node: node_id,
                    index: i,
                };
                let (prev_user, new_user_list) = match self.origin_data(origin).users.get() {
                    Some(UserIdList { first, last }) => {
                        match last {
                            UserId::In { node, index } if node == node_id => {
                                new_node_inputs[index].next_user.set(Some(new_in_id));
                            }
                            _ => {
                                self.user_data(last).next_user.set(Some(new_in_id));
                            }
                        }
                        let new_user_list = UserIdList {
                            first,
                            last: new_in_id,
                        };
                        (Some(last), new_user_list)
                    }
                    None => (
                        None, // No previous user.
                        UserIdList {
                            first: new_in_id,
                            last: new_in_id,
                        },
                    ),
                };
                self.origin_data(origin).users.set(Some(new_user_list));
                new_node_inputs.push(UserData {
                    origin,
                    sink: None,
                    prev_user: Cell::new(prev_user),
                    next_user: Cell::default(),
                });
            }

            let sig = kind.sig();

            self.nodes.borrow_mut().push(NodeData {
                ins: new_node_inputs,
                outs: vec![OriginData::default(); kind.sig().outs_len()],
                inner_regions: None,
                // FIXME replace with an argument from mk_node_with.
                outer_region: RegionId(0),
                kind,
            });

            assert_eq!(self.node_data(node_id).ins.len(), sig.ins_len());
            assert_eq!(self.node_data(node_id).outs.len(), sig.outs_len());

            node_id
        };

        let node_term = NodeTerm {
            kind: kind.clone(),
            origins: origins.into(),
        };

        let node_hash = self.hash_node_terms(&kind, origins);
        let mut interned_nodes = self.interned_nodes.borrow_mut();
        let entry = interned_nodes
            .raw_entry_mut()
            .from_key_hashed_nocheck(node_hash, &node_term);

        match entry {
            RawEntryMut::Occupied(e) => *e.get(),
            RawEntryMut::Vacant(e) => {
                let node_id = create_node(kind, origins);
                e.insert_hashed_nocheck(node_hash, node_term, node_id);
                node_id
            }
        }
    }

    fn mk_node(&self, op: S) -> Node<S>
    where
        S: Sig + Eq + Hash + Clone,
    {
        let node_id = self.mk_node_with(NodeKind::Op(op), &[]);
        Node {
            ctxt: self,
            id: node_id,
        }
    }

    fn node_builder(&self, op: S) -> NodeBuilder<S>
    where
        S: Sig,
    {
        NodeBuilder::new(self, NodeKind::Op(op))
    }

    fn node_ref(&self, node_id: NodeId) -> Node<S> {
        assert!(node_id.0 < self.nodes.borrow().len());
        Node {
            ctxt: self,
            id: node_id,
        }
    }

    fn user_ref<'g>(&'g self, user_id: UserId) -> User<'g, S> {
        match user_id {
            UserId::In { node, index } => assert!(index < self.node_data(node).ins.len()),
            UserId::Res { region, index } => assert!(index < self.region_data(region).res.len()),
        }

        User {
            ctxt: self,
            user_id,
        }
    }

    fn origin_ref<'g>(&'g self, origin_id: OriginId) -> Origin<'g, S> {
        match origin_id {
            OriginId::Out { node, index } => assert!(index < self.node_data(node).outs.len()),
            OriginId::Arg { region, index } => assert!(index < self.region_data(region).args.len()),
        }

        Origin {
            ctxt: self,
            origin_id,
        }
    }
}

impl<S> PartialEq for NodeCtxt<S> {
    fn eq(&self, other: &NodeCtxt<S>) -> bool {
        ptr::eq(self, other)
    }
}

struct NodeBuilder<'g, S> {
    ctxt: &'g NodeCtxt<S>,
    node_kind: NodeKind<S>,
    val_origins: Vec<ValOrigin<'g, S>>,
    st_origins: Vec<StOrigin<'g, S>>,
}

impl<'g, S: Sig> NodeBuilder<'g, S> {
    fn new(ctxt: &'g NodeCtxt<S>, node_kind: NodeKind<S>) -> NodeBuilder<'g, S> {
        let sig = node_kind.sig();
        NodeBuilder {
            ctxt,
            node_kind,
            val_origins: Vec::with_capacity(sig.val_ins),
            st_origins: Vec::with_capacity(sig.st_ins),
        }
    }

    fn operand(mut self, val_origin: ValOrigin<'g, S>) -> NodeBuilder<'g, S> {
        assert!(self.val_origins.len() < self.node_kind.sig().val_ins);
        self.val_origins.push(val_origin);
        self
    }

    fn operands(mut self, val_origins: &[ValOrigin<'g, S>]) -> NodeBuilder<'g, S>
    where
        S: Clone,
    {
        assert!(self.val_origins.is_empty());
        assert_eq!(self.node_kind.sig().val_ins, val_origins.len());
        self.val_origins.extend(val_origins.iter().cloned());
        self
    }

    fn state(mut self, st_origin: StOrigin<'g, S>) -> NodeBuilder<'g, S> {
        assert!(self.st_origins.len() < self.node_kind.sig().st_ins);
        self.st_origins.push(st_origin);
        self
    }

    fn states(mut self, st_origins: &[StOrigin<'g, S>]) -> NodeBuilder<'g, S>
    where
        S: Clone,
    {
        assert!(self.st_origins.is_empty());
        assert_eq!(self.node_kind.sig().st_ins, st_origins.len());
        self.st_origins.extend(st_origins.iter().cloned());
        self
    }

    fn finish(self) -> Node<'g, S>
    where
        S: Eq + Hash + Clone,
    {
        let sig = self.node_kind.sig();
        assert_eq!(self.val_origins.len(), sig.val_ins);
        assert_eq!(self.st_origins.len(), sig.st_ins);

        let origins: Vec<OriginId> = {
            let val_origins = self.val_origins.iter().map(|val_origin| val_origin.0.id());
            let st_origins = self.st_origins.iter().map(|st_origin| st_origin.0.id());
            val_origins.chain(st_origins).collect()
        };

        assert_eq!(origins.len(), sig.val_ins + sig.st_ins);

        let node_id = self.ctxt.mk_node_with(self.node_kind, &origins);

        Node {
            ctxt: self.ctxt,
            id: node_id,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Node<'g, S> {
    ctxt: &'g NodeCtxt<S>,
    id: NodeId,
}

impl<'g, S: fmt::Debug> fmt::Debug for Node<'g, S> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self.data().kind)
    }
}

impl<'g, S> Node<'g, S> {
    fn data(&self) -> Ref<'g, NodeData<S>> {
        self.ctxt.node_data(self.id)
    }
}

impl<'g, S: Sig + Copy> Node<'g, S> {
    fn val_in(&self, port: usize) -> ValUser<'g, S> {
        let sig = self.data().sig();
        assert!(port < sig.val_ins);
        ValUser(self.ctxt.user_ref(UserId::In {
            node: self.id,
            index: port,
        }))
    }

    fn val_out(&self, port: usize) -> ValOrigin<'g, S> {
        let sig = self.data().sig();
        assert!(port < sig.val_outs);
        ValOrigin(self.ctxt.origin_ref(OriginId::Out {
            node: self.id,
            index: port,
        }))
    }

    fn st_in(&self, port: usize) -> StUser<'g, S> {
        let sig = self.data().sig();
        assert!(port < sig.st_ins);
        StUser(self.ctxt.user_ref(UserId::In {
            node: self.id,
            index: sig.val_ins + port,
        }))
    }

    fn st_out(&self, port: usize) -> StOrigin<'g, S> {
        let sig = self.data().sig();
        assert!(port < sig.st_outs);
        StOrigin(self.ctxt.origin_ref(OriginId::Out {
            node: self.id,
            index: sig.val_outs + port,
        }))
    }
}

#[derive(Copy, Clone, PartialEq)]
struct User<'g, S> {
    ctxt: &'g NodeCtxt<S>,
    user_id: UserId,
}

impl<'g, S: fmt::Debug> fmt::Debug for User<'g, S> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self.user_id)
    }
}

impl<'g, S> User<'g, S> {
    fn id(&self) -> UserId {
        self.user_id
    }

    fn data(&self) -> Ref<'g, UserData> {
        self.ctxt.user_data(self.user_id)
    }

    fn origin(&self) -> Origin<'g, S> {
        let origin_id = self.data().origin;
        self.ctxt.origin_ref(origin_id)
    }
}

#[derive(Copy, Clone, PartialEq)]
struct Origin<'g, S> {
    ctxt: &'g NodeCtxt<S>,
    origin_id: OriginId,
}

impl<'g, S: fmt::Debug> fmt::Debug for Origin<'g, S> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self.origin_id)
    }
}

impl<'g, S> Origin<'g, S> {
    fn id(&self) -> OriginId {
        self.origin_id
    }

    fn data(&self) -> Ref<'g, OriginData> {
        self.ctxt.origin_data(self.origin_id)
    }

    fn users(&self) -> Users<'g, S> {
        let user_ref = |user_id| self.ctxt.user_ref(user_id);
        Users {
            first_and_last: self
                .data()
                .users
                .get()
                .map(|users| (user_ref(users.first), user_ref(users.last))),
        }
    }
}

struct Users<'g, S> {
    first_and_last: Option<(User<'g, S>, User<'g, S>)>,
}

impl<'g, S> Iterator for Users<'g, S> {
    type Item = User<'g, S>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.first_and_last.take() {
            Some((first, last)) => {
                if first.id() != last.id() {
                    if let Some(next_user) = first.data().next_user.get() {
                        self.first_and_last = Some((first.ctxt.user_ref(next_user), last));
                    }
                }
                Some(first)
            }
            None => None,
        }
    }
}

#[derive(Copy, Clone, PartialEq, Debug)]
struct ValUser<'g, S>(User<'g, S>);

impl<'g, S> ValUser<'g, S> {
    fn origin(&self) -> ValOrigin<'g, S> {
        ValOrigin(self.0.origin())
    }
}

#[derive(Copy, Clone, PartialEq, Debug)]
struct StUser<'g, S>(User<'g, S>);

impl<'g, S> StUser<'g, S> {
    fn origin(&self) -> StOrigin<'g, S> {
        StOrigin(self.0.origin())
    }
}

#[derive(Copy, Clone, PartialEq, Debug)]
struct ValOrigin<'g, S>(Origin<'g, S>);

impl<'g, S> ValOrigin<'g, S> {
    fn users(&self) -> impl Iterator<Item = ValUser<'g, S>> {
        self.0.users().map(ValUser)
    }
}

#[derive(Copy, Clone, PartialEq, Debug)]
struct StOrigin<'g, S>(Origin<'g, S>);

impl<'g, S> StOrigin<'g, S> {
    fn users(&self) -> impl Iterator<Item = StUser<'g, S>> {
        self.0.users().map(StUser)
    }
}

#[cfg(test)]
mod test {
    use super::{NodeCtxt, NodeKind, OriginId, Sig, SigS};

    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
    enum TestData {
        Lit(u32),
        Neg,
        St,
        BinAdd,
        LoadOffset,
        OpA,
        OpB,
        OpC,
    }

    impl Sig for TestData {
        fn sig(&self) -> SigS {
            match self {
                TestData::Lit(..) => SigS {
                    val_outs: 1,
                    ..SigS::default()
                },
                TestData::Neg | TestData::OpA | TestData::OpB | TestData::OpC => SigS {
                    val_ins: 1,
                    val_outs: 1,
                    ..SigS::default()
                },
                TestData::St => SigS {
                    st_outs: 1,
                    ..SigS::default()
                },
                TestData::BinAdd => SigS {
                    val_ins: 2,
                    val_outs: 1,
                    ..SigS::default()
                },
                TestData::LoadOffset => SigS {
                    val_ins: 2,
                    val_outs: 1,
                    st_ins: 1,
                    st_outs: 1,
                },
            }
        }
    }

    #[test]
    fn create_single_node() {
        let ncx = NodeCtxt::new();
        let n0 = ncx.mk_node_with(NodeKind::Op(TestData::Lit(0)), &[]);
        assert_eq!(0, ncx.node_data(n0).ins.len());
        assert_eq!(1, ncx.node_data(n0).outs.len());
    }

    #[test]
    fn create_node_with_an_input() {
        let ncx = NodeCtxt::new();
        let n0 = ncx.mk_node_with(NodeKind::Op(TestData::Lit(0)), &[]);
        let n1 = ncx.mk_node_with(
            NodeKind::Op(TestData::Neg),
            &[OriginId::Out { node: n0, index: 0 }],
        );

        assert_eq!(Some(n0), ncx.node_data(n1).ins[0].origin.node());
    }

    #[test]
    fn create_node_with_an_input_using_builder() {
        let ncx = NodeCtxt::new();

        let n0 = ncx.mk_node(TestData::Lit(0));
        let n1 = ncx
            .node_builder(TestData::Neg)
            .operand(n0.val_out(0))
            .finish();

        assert_eq!(Some(n0.id), n1.data().ins[0].origin.node());
        assert_eq!(n0.val_out(0), n1.val_in(0).origin());
    }

    #[test]
    fn create_node_with_input_ports() {
        let ncx = NodeCtxt::new();

        let n0 = ncx.mk_node_with(NodeKind::Op(TestData::Lit(2)), &[]);

        assert_eq!(0, ncx.node_data(n0).ins.len());
        assert_eq!(1, ncx.node_data(n0).outs.len());

        let n1 = ncx.mk_node_with(NodeKind::Op(TestData::Lit(3)), &[]);

        assert_eq!(0, ncx.node_data(n1).ins.len());
        assert_eq!(1, ncx.node_data(n1).outs.len());

        let n2 = ncx.mk_node_with(
            NodeKind::Op(TestData::BinAdd),
            &[
                OriginId::Out { node: n0, index: 0 },
                OriginId::Out { node: n1, index: 0 },
            ],
        );

        assert_eq!(2, ncx.node_data(n2).ins.len());
        assert_eq!(1, ncx.node_data(n2).outs.len());

        assert_eq!(
            Some(n2),
            ncx.node_data(n0).outs[0].users.get().unwrap().first.node()
        );
        assert_eq!(
            Some(n2),
            ncx.node_data(n0).outs[0].users.get().unwrap().last.node()
        );
        assert_eq!(
            Some(n2),
            ncx.node_data(n1).outs[0].users.get().unwrap().first.node()
        );
        assert_eq!(
            Some(n2),
            ncx.node_data(n1).outs[0].users.get().unwrap().last.node()
        );
    }

    #[test]
    fn create_node_operands_and_states_using_builder_single() {
        let ncx = NodeCtxt::new();

        let n0 = ncx.mk_node(TestData::Lit(2));
        let n1 = ncx.mk_node(TestData::Lit(3));
        let n2 = ncx.mk_node(TestData::St);

        let n3 = ncx
            .node_builder(TestData::LoadOffset)
            .operand(n0.val_out(0))
            .operand(n1.val_out(0))
            .state(n2.st_out(0))
            .finish();

        assert_eq!(n0.val_out(0), n3.val_in(0).origin());
        assert_eq!(n1.val_out(0), n3.val_in(1).origin());
        assert_eq!(n2.st_out(0), n3.st_in(0).origin());
    }

    #[test]
    fn create_node_operands_and_states_using_builder_slice() {
        let ncx = NodeCtxt::new();

        let n0 = ncx.mk_node(TestData::Lit(2));
        let n1 = ncx.mk_node(TestData::Lit(3));
        let n2 = ncx.mk_node(TestData::St);

        let n3 = ncx
            .node_builder(TestData::LoadOffset)
            .operands(&[n0.val_out(0), n1.val_out(0)])
            .states(&[n2.st_out(0)])
            .finish();

        assert_eq!(n0.val_out(0), n3.val_in(0).origin());
        assert_eq!(n1.val_out(0), n3.val_in(1).origin());
        assert_eq!(n2.st_out(0), n3.st_in(0).origin());
    }

    #[test]
    fn users_iterator() {
        let ncx = NodeCtxt::new();

        let n0 = ncx.mk_node(TestData::Lit(0));

        let n1 = ncx
            .node_builder(TestData::OpA)
            .operand(n0.val_out(0))
            .finish();

        let n2 = ncx
            .node_builder(TestData::OpB)
            .operand(n0.val_out(0))
            .finish();

        let n3 = ncx
            .node_builder(TestData::OpC)
            .operand(n0.val_out(0))
            .finish();

        let mut users = n0.val_out(0).users();

        assert_eq!(Some(n1.val_in(0)), users.next());
        assert_eq!(Some(n2.val_in(0)), users.next());
        assert_eq!(Some(n3.val_in(0)), users.next());
        assert_eq!(None, users.next());
    }

    #[test]
    fn reuse_existing_eq_nodes_at_creation() {
        let ncx = NodeCtxt::new();

        let n0 = ncx.mk_node(TestData::Lit(2));
        let n1 = ncx.mk_node(TestData::Lit(3));
        let n2 = ncx.mk_node(TestData::Lit(2));

        assert_eq!(n0.id, n2.id);
        assert_ne!(n0.id, n1.id);
        assert_ne!(n1.id, n2.id);

        let n3 = ncx
            .node_builder(TestData::BinAdd)
            .operand(n0.val_out(0))
            .operand(n1.val_out(0))
            .finish();

        let n4 = ncx
            .node_builder(TestData::BinAdd)
            .operand(n0.val_out(0))
            .operand(n2.val_out(0))
            .finish();

        let n5 = ncx
            .node_builder(TestData::BinAdd)
            .operand(n2.val_out(0))
            .operand(n1.val_out(0))
            .finish();

        assert_ne!(n3.id, n4.id);
        assert_ne!(n4.id, n5.id);
        assert_eq!(n3.id, n5.id);
    }
}
