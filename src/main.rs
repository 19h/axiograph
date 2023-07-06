use std::collections::HashSet;
use std::error::Error;
use std::fs::File;
use std::future::Future;
use std::io::{Read, Write};
use std::sync::Arc;
use async_recursion::async_recursion;
use clap::{Parser, Arg, Command};
use colored::Colorize;
use evilcorp_secondpilot::{CompletionRequestBuilder, EvilcorpSecondPilotClient, Message};

use futures::future::join_all;

#[derive(Parser)]
struct AxgArgs {
    #[arg(long, short = 'd', default_value="1", value_parser=|val: &str| val.parse::<u32>())]
    depth: u32,
    #[arg(long, short = 'c')]
    context: Option<String>,
    pub axiom: String,
    #[arg(long, env, short = 't')]
    pub token: String,
}

#[derive(Clone)]
pub struct Node {
    pub axiom: String,
    pub children: Vec<Box<Node>>,
}

impl Node {
    fn new(axiom: String) -> Self {
        Self {
            axiom,
            children: Vec::new(),
        }
    }

    fn to_dot(&self) -> String {
        let mut dot = String::new();

        dot.push_str(&format!("\"{}\" [label=\"{}\"];\n", self.axiom, self.axiom));

        for child in &self.children {
            dot.push_str(&format!("\"{}\" -> \"{}\";\n", self.axiom, child.axiom));
            dot.push_str(&child.to_dot());
        }

        dot
    }

    fn to_plain(&self) -> String {
        let mut dot = String::new();

        dot.push_str(&format!("{}\n", self.axiom));

        for child in &self.children {
            dot.push_str(&child.to_plain());
        }

        dot
    }
}

pub async fn get_axiom(
    mut client: EvilcorpSecondPilotClient,
    axiom: &str,
    previous_axiom: Vec<String>,
) -> Result<Vec<String>, Box<dyn Error>> {
    let messages = vec![
        Message {
            role: "system".to_string(),
            content: include_str!("../system.prompt.txt").to_string(),
        },
        Message {
            role: "user".to_string(),
            content: r#"I want you to write four connected and related and semantically relevant axioms to the axiom: "USA"

You must respond with nothing but syntactically correct JSON matching the format of the following example:

Example:

[
      "The invasion was based on the belief that Iraq possessed weapons of mass destruction.",
      "The invasion was controversial and sparked widespread protests and opposition.",
      "The invasion led to the overthrow of Saddam Hussein's regime."
]

The axioms must be concise and relevant."#.to_string(),
        },
        Message {
            role: "assistant".to_string(),
            content: r#"[
  "USA is a federal republic.",
  "USA is the third largest country.",
  "USA is a global superpower."
]"#.to_string(),
        },
        Message {
            role: "user".to_string(),
            content: format!(
                r#"I want you to write four connected and related and semantically relevant axioms to the axiom: "{}"
{}

You must respond with nothing but syntactically correct JSON matching the format of the following example:

Example:

[
      "The invasion was based on the belief that Iraq possessed weapons of mass destruction.",
      "The invasion was controversial and sparked widespread protests and opposition.",
      "The invasion led to the overthrow of Saddam Hussein's regime."
]

The axioms must be concise and relevant."#,
                axiom,
                match previous_axiom.len() {
                    0 => "".to_string(),
                    _ => format!(
                        r#"The previous axioms were: {}."#,
                        previous_axiom
                            .iter()
                            .map(|axiom|
                                format!("\"{}\"", axiom)
                            )
                            .collect::<Vec<String>>()
                            .join(", "),
                    ),
                },
            ),
        },
    ];

    let request =
        CompletionRequestBuilder::new()
            .with_model("copilot-chat".to_string())
            .with_temperature(0.7)
            .with_messages(messages)
            .with_top_p(1)
            .with_n(1)
            .build()
            .await;

    let mut i = 0;
    let max_attempts = 10;

    loop {
        let response =
            client
                .query(
                    &request,
                )
                .await?;

        match serde_json::from_str::<'_, Vec<String>>(&response) {
            Ok(response) => {
                break Ok(response);
            },
            Err(err) => {
                dbg!(err);

                if i >= max_attempts {
                    break Err("Failed to parse response".into());
                }

                i += 1;

                // wait for a random time between 5 and 20 seconds
                let wait_time = rand::random::<u64>() % 15 + 5;

                println!("Waiting for {} seconds", wait_time);

                tokio::time::sleep(tokio::time::Duration::from_secs(wait_time)).await;
            },
        }
    }
}

#[async_recursion::async_recursion]
pub async fn build_axiom(
    client: EvilcorpSecondPilotClient,
    axiom: String,
    previous_axiom: Vec<String>,
    level: u32,
    max_depth: u32,
) -> Option<Box<Node>> {
    println!("[{}:{}] {}", level, max_depth, axiom.bold().red());

    let mut node = Node::new(axiom.to_string());

    if level >= max_depth {
        return Some(Box::new(node));
    }

    let new_axioms =
        get_axiom(
            client.clone(),
            axiom.as_str(),
            previous_axiom.clone(),
        )
        .await
            .ok()?;

    let mut children_futures = Vec::new();

    for new_axiom in new_axioms {
        let axiom = axiom.clone();
        let previous_axiom = previous_axiom.clone();

        let client = client.clone();

        let child_future = tokio::spawn(async move {
            build_axiom(
                client,
                new_axiom.clone(),
                [previous_axiom.as_slice(), vec![axiom].as_slice()].concat(),
                level + 1,
                max_depth,
            )
            .await
        });

        children_futures.push(child_future);
    }

    let children_results = join_all(children_futures).await;

    for child_result in children_results {
        if let Some(child) = child_result.ok()?.as_ref() {
            node.children.push((*child).clone());
        }
    }

    Some(Box::new(node))
}

async fn sum_chunk(
    client: EvilcorpSecondPilotClient,
    chunk: String,
) -> Option<String> {
    let messages = vec![
        Message {
            role: "system".to_string(),
            content: include_str!("../system.prompt.txt").to_string(),
        },
        Message {
            role: "user".to_string(),
            content: format!(
                r#"Given the following list, extract all unique key-points expressed as axioms in the form of a list - do not repeat any point:

----

{}

----

Instead of repeating references to a subject, group all statements by subject and prefix the subsquent axioms with a leading minus on their respective line.

Example:

Instead of:

"One-party democracy can result in corruption and abuse of power.",
"One-party democracies may use censorship to maintain control over the narrative."
"One-party democracy can lead to nepotism and favoritism.",
"One-party democracies may suppress minority rights."

You should write:

One-party democracy:
- can result in corruption and abuse of power.
- may use censorship to maintain control over the narrative.
- can lead to nepotism and favoritism.
- may suppress minority rights.
",
"#,


                chunk),
        },
    ];

    let request =
        CompletionRequestBuilder::new()
            .with_model("copilot-chat".to_string())
            .with_temperature(0.7)
            .with_messages(messages)
            .with_top_p(1)
            .with_n(1)
            .build()
            .await;

    client
        .clone()
        .query(&request)
        .await
        .ok()
}

#[async_recursion::async_recursion]
async fn sumsum(
    client: EvilcorpSecondPilotClient,
    text: String,
) -> String {
    let plain = text.lines().collect::<HashSet<_>>();
    let plain = plain.iter().map(|s| s.to_string()).collect::<Vec<_>>();

    let mut summary_chunks = vec![];
    let mut chunk = String::new();

    for line in plain {
        chunk.push_str(format!("{}\n", line).as_str());

        if chunk.len() > 8000 {
            summary_chunks.push(chunk);
            chunk = String::new();
        }
    }

    if !chunk.is_empty() {
        summary_chunks.push(chunk);
    }

    let mut summary_futures = vec![];

    for chunk in summary_chunks {
        let client = client.clone();

        let child_future = tokio::spawn(async move {
            sum_chunk(client, chunk)
                .await
        });

        summary_futures.push(child_future);
    }

    let summarized_chunks =
        join_all(
            summary_futures,
        )
        .await
        .into_iter()
        .map(|r| r.ok())
        .flatten()
        .flatten()
        .collect::<Vec<_>>();

    let summarized_chunks = summarized_chunks.as_slice().join("\n");

    if summarized_chunks.len() > 8000 {
        return sumsum(
            client.clone(),
            summarized_chunks,
        ).await;
    }

    summarized_chunks
}

struct Axiograph {
    pub axiom: String,
    pub context: Option<String>,
    pub depth: u32,

    pub axiom_graph: Option<Box<Node>>,

    client: EvilcorpSecondPilotClient,
}

impl Axiograph {
    pub fn new(
        token: String,
        root_axiom: String,
        max_depth: u32,
        context: Option<String>,
    ) -> Self {
        let client =
            EvilcorpSecondPilotClient::new(
                token,
            );

        Self {
            axiom: root_axiom,
            axiom_graph: None,
            context,
            depth: max_depth,
            client,
        }
    }

    pub async fn run(
        &mut self,
    ) {
        self.axiom_graph = build_axiom(
            self.client.clone(),
            self.axiom.clone(),
            Vec::new(),
            0,
            self.depth,
        ).await;
    }

    pub fn get_axioms(&self) -> Option<&Node> {
        self.axiom_graph.as_ref().map(|n| n.as_ref())
    }

    pub fn to_plain(&self) -> String {
        self.axiom_graph.as_ref().unwrap().to_plain()
    }

    pub fn to_dot(&self) -> String {
        node_to_dot(self.axiom_graph.as_ref().unwrap())
    }

    pub async fn summarize(&mut self) -> String {
        let plain = self.to_plain();

        println!("Established {} axioms", plain.lines().count());

        let sum_key_points = sumsum(
            self.client.clone(),
            plain,
        ).await;

        let messages = vec![
            Message {
                role: "user".to_string(),
                content: format!(
                    r#"I want you to summarize the following text as detailed and elaborate as possible. Then provide all keypoints in the form of a comprehensive list.

{}"#,
                    sum_key_points),
            },
        ];

        let request =
            CompletionRequestBuilder::new()
                .with_model("copilot-chat".to_string())
                .with_temperature(0.7)
                .with_messages(messages)
                .with_top_p(1)
                .with_n(1)
                .build()
                .await;

        match self.client
            .clone()
            .query(&request)
            .await {
            Ok(summarized_chunk) => {
                summarized_chunk
            },
            Err(_) => {
                return String::new();
            },
        }
    }
}

fn node_to_dot(node: &Node) -> String {
    format!("digraph axioms {{rankdir=LR;overlap=prism;overlap_scaling=-5;\n{}\n}}", node.to_dot())
}

#[tokio::main]
async fn main() {
    let args = AxgArgs::parse();

    let depth = args.depth;
    let axiom = args.axiom;
    let context = args.context;

    println!("Depth: {}", depth);
    println!("Axiom: {}", axiom);

    if context.is_some() {
        println!("Context: {}", context.as_ref().unwrap());
    } else {
        println!();
    }

    // if context is some, read the file
    let context =
        context
        .as_ref()
        .map(|path| {
            let mut file = File::open(path).unwrap();
            let mut context = String::new();
            file.read_to_string(&mut context).unwrap();
            context
        });

    let mut axg =
        Axiograph::new(
            args.token,
            axiom,
            depth,
            context,
        );

    axg.run().await;

    let axioms = axg.get_axioms().unwrap();

    {
        println!("Writing axioms to axioms.txt...");

        let plain = axioms.to_plain();
        let plain = plain.lines().collect::<HashSet<_>>();
        let plain = plain.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let plain = plain.join("\n");

        // write to file
        let mut file = File::create("axioms.txt").unwrap();

        file.write_all(plain.as_bytes()).unwrap();
    }

    {
        println!("Writing dot file...");

        let dotfile = node_to_dot(&axioms);

        // write to file
        let mut file = File::create("axioms.dot").unwrap();

        file.write_all(dotfile.as_bytes()).unwrap();

        // run dot

        let output = std::process::Command::new("sfdp")
            .arg("-Tsvg")
            .arg("axioms.dot")
            .arg("-o")
            .arg("axioms.svg")
            .output()
            .expect("failed to execute process");

        println!("dot status: {}", output.status);
    }

    {
        println!("Summarizing...");

        let plain = axg.summarize().await;

        let mut file = File::create("axioms.summary.txt").unwrap();

        file.write_all(plain.as_bytes()).unwrap();

        println!("Summary:\n\n{}", plain.bold().yellow());
    }
}
