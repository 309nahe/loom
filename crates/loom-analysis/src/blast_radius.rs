//! Blast radius calculation and impact analysis engine.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use loom_core::id::SymbolId;
use loom_core::symbol::SymbolNode;
use loom_graph::CodeGraph;

/// Risk category assessing modification danger of a symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RiskLevel {
    /// Safe / localized changes.
    Low,
    /// Moderate impact, few internal callers.
    Medium,
    /// Significant impact across multiple modules or public APIs.
    High,
    /// Extreme impact: public APIs, wide surface, or critical entrypoint.
    Critical,
}

/// Detailed context for a direct caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectCallerDetail {
    /// Symbol ID of caller.
    pub id: SymbolId,
    /// Name of caller symbol.
    pub name: String,
    /// File path where caller is defined.
    pub file_path: PathBuf,
    /// Line number of the call site.
    pub call_site_line: u32,
    /// Whether the caller is exported.
    pub is_exported: bool,
}

/// Detailed context for a transitive (upstream) caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitiveCallerDetail {
    /// Symbol ID of transitive caller.
    pub id: SymbolId,
    /// Name of caller symbol.
    pub name: String,
    /// File path where caller is defined.
    pub file_path: PathBuf,
    /// Topological depth from the target symbol.
    pub depth: usize,
    /// Whether the caller is exported.
    pub is_exported: bool,
}

/// Test function associated with the target symbol via call paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssociatedTestDetail {
    /// Symbol ID of test.
    pub id: SymbolId,
    /// Name of test function.
    pub name: String,
    /// File path of test file.
    pub file_path: PathBuf,
    /// Line number where test function is declared.
    pub call_site_line: u32,
}

/// Blast radius report summarizing upstream impact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlastRadiusReport {
    /// Target symbol being analyzed.
    pub target_symbol: SymbolNode,
    /// Total number of unique affected upstream symbols.
    pub total_affected_symbols: usize,
    /// Direct callers.
    pub direct_callers: Vec<DirectCallerDetail>,
    /// Transitive callers (depth >= 2).
    pub transitive_callers: Vec<TransitiveCallerDetail>,
    /// Tests transitively invoking the target symbol.
    pub associated_tests: Vec<AssociatedTestDetail>,
    /// Computed risk assessment level.
    pub risk_level: RiskLevel,
    /// Numerical risk score.
    pub risk_score_raw: f64,
    /// Human-readable explanation of risk level.
    pub risk_rationale: String,
}

/// Blast radius computation engine.
#[derive(Debug, Clone)]
pub struct BlastRadiusCalculator<'a> {
    graph: &'a CodeGraph,
}

impl<'a> BlastRadiusCalculator<'a> {
    /// Creates a new calculator over a reference to `CodeGraph`.
    #[must_use]
    pub const fn new(graph: &'a CodeGraph) -> Self {
        Self { graph }
    }

    /// Computes the complete blast radius report for a given target symbol ID.
    #[must_use]
    pub fn calculate(&self, target_id: &SymbolId, max_depth: usize) -> Option<BlastRadiusReport> {
        let target_node = self.graph.get_symbol(target_id)?.clone();

        // 1. Direct callers
        let direct_callers_raw = self.graph.get_callers(target_id);
        let mut direct_callers = Vec::with_capacity(direct_callers_raw.len());
        for (node, edge) in direct_callers_raw {
            direct_callers.push(DirectCallerDetail {
                id: node.id,
                name: node.name.clone(),
                file_path: node.file_path.clone(),
                call_site_line: edge.call_site_line,
                is_exported: node.is_exported,
            });
        }

        // 2. Transitive callers
        let transitive_raw = self.graph.find_transitive_callers(target_id, max_depth);
        let mut transitive_callers = Vec::new();
        let mut associated_tests = Vec::new();

        let mut risk_score = 0.0;
        let mut public_api_impacted = target_node.is_exported;

        if target_node.is_exported {
            risk_score += 15.0;
        }

        for (node, depth) in transitive_raw {
            if is_test_symbol(&node) {
                associated_tests.push(AssociatedTestDetail {
                    id: node.id,
                    name: node.name.clone(),
                    file_path: node.file_path.clone(),
                    call_site_line: node.line_range.0,
                });
            } else if depth > 1 {
                transitive_callers.push(TransitiveCallerDetail {
                    id: node.id,
                    name: node.name.clone(),
                    file_path: node.file_path.clone(),
                    depth,
                    is_exported: node.is_exported,
                });
            }

            if node.is_exported {
                public_api_impacted = true;
            }

            // Weight calculation: weight(u, v) * criticality(u) with depth decay (0.75^depth)
            let decay = 0.75_f64.powi(i32::try_from(depth).unwrap_or(1));
            let criticality = if node.is_exported { 3.0 } else { 1.0 };
            risk_score += decay * criticality * 2.0;
        }

        let total_affected = direct_callers.len() + transitive_callers.len();

        // Determine Risk Level & Rationale
        let (risk_level, risk_rationale) = if public_api_impacted {
            if risk_score >= 20.0 || total_affected >= 5 {
                (
                    RiskLevel::Critical,
                    format!(
                        "CRITICAL: Modifying exported interface impacting {total_affected} total symbols across public boundary"
                    ),
                )
            } else if risk_score >= 8.0 || total_affected >= 2 {
                (
                    RiskLevel::High,
                    format!(
                        "HIGH: Significant impact on public API surface ({total_affected} callers)"
                    ),
                )
            } else {
                (
                    RiskLevel::Medium,
                    format!(
                        "MEDIUM: Public API symbol with localized callers ({total_affected} callers)"
                    ),
                )
            }
        } else if risk_score >= 15.0 || total_affected >= 6 {
            (
                RiskLevel::High,
                format!(
                    "HIGH: Wide internal blast radius across modules ({total_affected} affected symbols)"
                ),
            )
        } else if risk_score >= 3.0 || total_affected >= 1 {
            (
                RiskLevel::Medium,
                format!(
                    "MEDIUM: Moderate internal impact ({total_affected} callers in repository)"
                ),
            )
        } else {
            (
                RiskLevel::Low,
                "LOW: Localized internal symbol with 0 upstream callers".to_string(),
            )
        };

        Some(BlastRadiusReport {
            target_symbol: target_node,
            total_affected_symbols: total_affected,
            direct_callers,
            transitive_callers,
            associated_tests,
            risk_level,
            risk_score_raw: risk_score,
            risk_rationale,
        })
    }
}

/// Helper function to detect if a symbol belongs to a test suite.
#[must_use]
pub fn is_test_symbol(symbol: &SymbolNode) -> bool {
    let name_lower = symbol.name.to_lowercase();
    let path_str = symbol.file_path.to_string_lossy().to_lowercase();

    name_lower.starts_with("test_")
        || name_lower.ends_with("_test")
        || name_lower.starts_with("test")
        || path_str.contains("test")
        || path_str.contains("tests")
        || path_str.contains("__tests__")
}

#[cfg(test)]
mod tests {
    use super::*;
    use loom_core::edge::{DependencyEdge, EdgeKind};
    use loom_core::symbol::SymbolKind;

    fn make_test_node(file: &str, name: &str, is_exported: bool) -> SymbolNode {
        let id = SymbolId::derive(file, &[], name, &format!("fn {name}()"));
        SymbolNode::new(
            id,
            name,
            SymbolKind::Function,
            file,
            (0, 50),
            (1, 10),
            None,
            format!("fn {name}()"),
            is_exported,
            1,
        )
    }

    #[test]
    fn test_blast_radius_calculation_and_risk_scoring() {
        let mut graph = CodeGraph::new();

        let target = make_test_node("src/core/auth.rs", "validate_secret", false);
        let caller1 = make_test_node("src/api/auth.rs", "login_endpoint", true);
        let caller2 = make_test_node("src/api/routes.rs", "router", true);
        let test_node = make_test_node("tests/auth_tests.rs", "test_login", false);

        let id_target = target.id;
        let id_caller1 = caller1.id;
        let id_caller2 = caller2.id;
        let id_test = test_node.id;

        graph.upsert_symbol(target);
        graph.upsert_symbol(caller1);
        graph.upsert_symbol(caller2);
        graph.upsert_symbol(test_node);

        let edge = DependencyEdge::new(EdgeKind::Calls, 5, false);
        // caller2 -> caller1 -> target
        // test_node -> target
        graph
            .add_edge(id_caller1, id_target, edge.clone())
            .expect("add edge 1");
        graph
            .add_edge(id_caller2, id_caller1, edge.clone())
            .expect("add edge 2");
        graph
            .add_edge(id_test, id_target, edge)
            .expect("add test edge");

        let calculator = BlastRadiusCalculator::new(&graph);
        let report = calculator
            .calculate(&id_target, 5)
            .expect("report generated");

        assert_eq!(report.target_symbol.name, "validate_secret");
        assert_eq!(report.direct_callers.len(), 2); // caller1 + test_node in direct incoming
        assert_eq!(report.associated_tests.len(), 1);
        assert_eq!(report.associated_tests[0].name, "test_login");
        assert_eq!(report.transitive_callers.len(), 1);
        assert_eq!(report.transitive_callers[0].name, "router");
        assert!(matches!(
            report.risk_level,
            RiskLevel::High | RiskLevel::Critical
        ));
    }
}
