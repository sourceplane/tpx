use anyhow::{Context as AnyhowContext, Result, anyhow, bail};
use petgraph::{
    Direction,
    graph::{DiGraph, NodeIndex},
};
use std::collections::{BTreeSet, HashMap};
use tpx_parser::{InternalModel, Tool};

pub const CRATE_NAME: &str = "tpx-core";
pub const STAGE: &str = "stage-2";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Context {
    pub model: InternalModel,
}

impl Context {
    pub fn new(model: InternalModel) -> Self {
        Self { model }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
/// Ordered batches of tools where each batch can execute in parallel.
pub struct ExecutionPlan {
    pub steps: Vec<ExecutionStep>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
/// A deterministic set of tools whose dependencies have already been satisfied.
pub struct ExecutionStep {
    pub tools: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DependencyGraph {
    graph: DiGraph<String, ()>,
    node_indices: HashMap<String, NodeIndex>,
    root: String,
}

impl DependencyGraph {
    pub fn root(&self) -> &str {
        &self.root
    }

    pub fn tool_names(&self) -> Vec<String> {
        let mut names = self.node_indices.keys().cloned().collect::<Vec<_>>();
        names.sort();
        names
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Engine;

impl Engine {
    pub const fn new() -> Self {
        Self
    }

    pub const fn crate_name(&self) -> &'static str {
        CRATE_NAME
    }

    pub const fn stage(&self) -> &'static str {
        STAGE
    }

    pub fn run(&self, tool: &str, args: &[String], ctx: &Context) -> Result<i32> {
        run(tool, args, ctx)
    }

    pub fn build_execution_plan(&self, tool: &str, ctx: &Context) -> Result<ExecutionPlan> {
        build_execution_plan(tool, ctx)
    }
}

pub fn run(tool: &str, args: &[String], ctx: &Context) -> Result<i32> {
    let _ = args;
    let _resolved_tool = resolve_tool(tool, ctx)
        .with_context(|| format!("failed to resolve execution target '{tool}'"))?;
    let _plan = build_execution_plan(tool, ctx)
        .with_context(|| format!("failed to build execution plan for '{tool}'"))?;

    Ok(0)
}

pub fn resolve_tool<'a>(name: &str, ctx: &'a Context) -> Result<&'a Tool> {
    let tool_name = normalize_tool_name(name)?;

    ctx.model
        .tools
        .get(tool_name)
        .with_context(|| format!("tool '{tool_name}' was not found"))
}

pub fn resolve_dependencies<'a>(tool: &str, ctx: &'a Context) -> Result<Vec<&'a Tool>> {
    let root_name = normalize_tool_name(tool)?.to_owned();
    let plan = build_execution_plan(&root_name, ctx)?;
    let dependency_names = plan
        .steps
        .into_iter()
        .flat_map(|step| step.tools)
        .filter(|name| name != &root_name)
        .collect::<Vec<_>>();

    dependency_names
        .iter()
        .map(|name| resolve_tool(name, ctx))
        .collect()
}

pub fn build_dependency_graph(tool: &str, ctx: &Context) -> Result<DependencyGraph> {
    let root = normalize_tool_name(tool)?.to_owned();
    let mut graph = DiGraph::new();
    let mut node_indices = HashMap::new();
    let mut visit_states = HashMap::new();
    let mut traversal_stack = Vec::new();

    visit_tool(
        &root,
        ctx,
        &mut graph,
        &mut node_indices,
        &mut visit_states,
        &mut traversal_stack,
    )?;

    Ok(DependencyGraph {
        graph,
        node_indices,
        root,
    })
}

pub fn build_execution_plan(tool: &str, ctx: &Context) -> Result<ExecutionPlan> {
    let dependency_graph = build_dependency_graph(tool, ctx)?;
    let mut pending_incoming = dependency_graph
        .node_indices
        .iter()
        .map(|(name, node_index)| {
            (
                name.clone(),
                dependency_graph
                    .graph
                    .neighbors_directed(*node_index, Direction::Incoming)
                    .count(),
            )
        })
        .collect::<HashMap<_, _>>();
    let mut ready = pending_incoming
        .iter()
        .filter_map(|(name, incoming_count)| {
            if *incoming_count == 0 {
                Some(name.clone())
            } else {
                None
            }
        })
        .collect::<BTreeSet<_>>();
    let mut processed = 0usize;
    let mut steps = Vec::new();

    while !ready.is_empty() {
        let current_batch = ready.into_iter().collect::<Vec<_>>();
        let mut next_ready = BTreeSet::new();

        processed += current_batch.len();

        for tool_name in &current_batch {
            let node_index = *dependency_graph
                .node_indices
                .get(tool_name)
                .ok_or_else(|| {
                    anyhow!("tool '{tool_name}' is missing from the dependency graph")
                })?;

            for dependent_index in dependency_graph
                .graph
                .neighbors_directed(node_index, Direction::Outgoing)
            {
                let dependent_name = dependency_graph.graph[dependent_index].clone();
                let incoming_count =
                    pending_incoming.get_mut(&dependent_name).ok_or_else(|| {
                        anyhow!("tool '{dependent_name}' is missing dependency state")
                    })?;

                *incoming_count -= 1;

                if *incoming_count == 0 {
                    next_ready.insert(dependent_name);
                }
            }
        }

        steps.push(ExecutionStep {
            tools: current_batch,
        });
        ready = next_ready;
    }

    if processed != dependency_graph.node_indices.len() {
        bail!("cyclic dependency detected while building execution plan");
    }

    Ok(ExecutionPlan { steps })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisitState {
    Visiting,
    Visited,
}

fn visit_tool(
    tool_name: &str,
    ctx: &Context,
    graph: &mut DiGraph<String, ()>,
    node_indices: &mut HashMap<String, NodeIndex>,
    visit_states: &mut HashMap<String, VisitState>,
    traversal_stack: &mut Vec<String>,
) -> Result<()> {
    match visit_states.get(tool_name).copied() {
        Some(VisitState::Visited) => return Ok(()),
        Some(VisitState::Visiting) => {
            let cycle_start = traversal_stack
                .iter()
                .position(|entry| entry == tool_name)
                .unwrap_or(0);
            let mut cycle = traversal_stack[cycle_start..].to_vec();
            cycle.push(tool_name.to_owned());

            bail!("cyclic dependency detected: {}", cycle.join(" -> "));
        }
        None => {}
    }

    let current_tool = resolve_tool(tool_name, ctx)?;
    let current_node = ensure_node(tool_name, graph, node_indices);
    let dependencies = dependency_names(current_tool)?;

    visit_states.insert(tool_name.to_owned(), VisitState::Visiting);
    traversal_stack.push(tool_name.to_owned());

    for dependency_name in dependencies {
        visit_tool(
            &dependency_name,
            ctx,
            graph,
            node_indices,
            visit_states,
            traversal_stack,
        )?;

        let dependency_node = ensure_node(&dependency_name, graph, node_indices);
        graph.add_edge(dependency_node, current_node, ());
    }

    traversal_stack.pop();
    visit_states.insert(tool_name.to_owned(), VisitState::Visited);

    Ok(())
}

fn ensure_node(
    tool_name: &str,
    graph: &mut DiGraph<String, ()>,
    node_indices: &mut HashMap<String, NodeIndex>,
) -> NodeIndex {
    if let Some(node_index) = node_indices.get(tool_name) {
        return *node_index;
    }

    let node_index = graph.add_node(tool_name.to_owned());
    node_indices.insert(tool_name.to_owned(), node_index);

    node_index
}

fn dependency_names(tool: &Tool) -> Result<Vec<String>> {
    let mut dependencies = BTreeSet::new();

    for dependency in &tool.spec.dependencies {
        dependencies.insert(normalize_tool_name(dependency)?.to_owned());
    }

    Ok(dependencies.into_iter().collect())
}

fn normalize_tool_name(name: &str) -> Result<&str> {
    let trimmed = name.trim();

    if trimmed.is_empty() {
        bail!("tool name cannot be empty");
    }

    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpx_parser::{Metadata, ToolSpec};

    #[test]
    fn exposes_core_metadata() {
        let engine = Engine::new();

        assert_eq!(engine.crate_name(), "tpx-core");
        assert_eq!(engine.stage(), "stage-2");
    }

    #[test]
    fn resolves_simple_dependency_chain() {
        let ctx = context_with_tools([
            ("curl", vec![]),
            ("helm", vec!["curl"]),
            ("kubectl", vec!["helm"]),
        ]);

        let plan = build_execution_plan("kubectl", &ctx).expect("plan should build");
        let dependency_names = resolve_dependencies("kubectl", &ctx)
            .expect("dependencies should resolve")
            .into_iter()
            .map(tool_name)
            .collect::<Vec<_>>();

        assert_eq!(
            plan_steps(&plan),
            vec![vec!["curl"], vec!["helm"], vec!["kubectl"]]
        );
        assert_eq!(dependency_names, vec!["curl", "helm"]);
    }

    #[test]
    fn resolves_parallel_graph_into_batches() {
        let ctx = context_with_tools([
            ("cache", vec![]),
            ("db", vec![]),
            ("api", vec!["db", "cache"]),
        ]);

        let graph = build_dependency_graph("api", &ctx).expect("graph should build");
        let plan = build_execution_plan("api", &ctx).expect("plan should build");

        assert_eq!(graph.root(), "api");
        assert_eq!(graph.tool_names(), vec!["api", "cache", "db"]);
        assert_eq!(plan_steps(&plan), vec![vec!["cache", "db"], vec!["api"]]);
    }

    #[test]
    fn fails_on_dependency_cycles() {
        let ctx = context_with_tools([("a", vec!["b"]), ("b", vec!["c"]), ("c", vec!["a"])]);

        let error = build_execution_plan("a", &ctx).expect_err("cycles should fail");

        assert!(error.to_string().contains("cyclic dependency detected"));
    }

    #[test]
    fn fails_fast_when_dependency_is_missing() {
        let ctx = context_with_tools([("kubectl", vec!["helm"])]);

        let error = build_execution_plan("kubectl", &ctx)
            .expect_err("missing dependencies should fail during graph resolution");

        assert!(error.to_string().contains("tool 'helm' was not found"));
    }

    #[test]
    fn deduplicates_shared_dependencies_in_dependency_order() {
        let ctx = context_with_tools([
            ("curl", vec![]),
            ("helm", vec!["curl"]),
            ("kustomize", vec!["curl"]),
            ("kubectl", vec!["helm", "kustomize"]),
        ]);

        let dependency_names = resolve_dependencies("kubectl", &ctx)
            .expect("dependencies should resolve")
            .into_iter()
            .map(tool_name)
            .collect::<Vec<_>>();
        let plan = build_execution_plan("kubectl", &ctx).expect("plan should build");

        assert_eq!(dependency_names, vec!["curl", "helm", "kustomize"]);
        assert_eq!(
            plan_steps(&plan),
            vec![vec!["curl"], vec!["helm", "kustomize"], vec!["kubectl"]]
        );
    }

    #[test]
    fn run_validates_graph_and_returns_success() {
        let ctx = context_with_tools([("kubectl", vec!["helm"]), ("helm", vec![])]);
        let args = vec!["version".to_owned(), "--client".to_owned()];

        let exit_code = run("kubectl", &args, &ctx).expect("valid plans should succeed");

        assert_eq!(exit_code, 0);
    }

    fn context_with_tools<const N: usize>(entries: [(&str, Vec<&str>); N]) -> Context {
        let tools = entries
            .into_iter()
            .map(|(name, dependencies)| (name.to_owned(), tool(name, dependencies)))
            .collect::<HashMap<_, _>>();

        Context::new(InternalModel {
            tools,
            ..InternalModel::default()
        })
    }

    fn tool(name: &str, dependencies: Vec<&str>) -> Tool {
        Tool {
            metadata: Some(Metadata {
                name: name.to_owned(),
            }),
            spec: ToolSpec {
                dependencies: dependencies.into_iter().map(str::to_owned).collect(),
                ..ToolSpec::default()
            },
        }
    }

    fn tool_name(tool: &Tool) -> String {
        tool.metadata
            .as_ref()
            .map(|metadata| metadata.name.clone())
            .expect("tool metadata should contain a name")
    }

    fn plan_steps(plan: &ExecutionPlan) -> Vec<Vec<&str>> {
        plan.steps
            .iter()
            .map(|step| step.tools.iter().map(String::as_str).collect())
            .collect()
    }
}
