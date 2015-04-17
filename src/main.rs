#![feature(plugin)]
#![plugin(docopt_macros)]
#![feature(core)]
#![feature(path_ext)]

extern crate core;
extern crate csv;
extern crate rustc_serialize;
extern crate docopt;
extern crate threadpool;
extern crate toml;

use std::io;
use std::io::Read;
use std::env::{set_current_dir, current_dir, args};
use std::fs::{File, read_dir, OpenOptions};
use std::process;
use std::process::Command;
use std::fs::PathExt;
use core::slice::SliceExt;
use std::sync::mpsc::channel;
use threadpool::ThreadPool;
use std::io::BufReader;
use std::io::BufRead;
use std::io::Cursor;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

docopt!(Args derive Debug Clone, "
Usage: grade [-n NUM] [-t TEMPLATE] <material-path> <command>
       grade --help

Options:
  -h, --help       Show this message.
  -n COUNT         Truncate the output to LINE_COUNT
  -t TEMPLATE      Use a CSV template file
", flag_n: Option<usize>, flag_t: Option<String>);

fn in_directory<F>(path: &Path, block: F)
    where F : Fn() {
    let directory = current_dir().unwrap();
    set_current_dir(path).unwrap();
    block();
    set_current_dir(&directory).unwrap()
}

fn copy_grading_materials(args: &Args, files: Vec<PathBuf>) -> io::Result<()> {
    let working_path = current_dir().unwrap();
    let grading_path = working_path.join(&args.arg_material_path);
    let mut cp = process::Command::new("cp");
    cp.arg("-rf");
    for file in files {
        cp.arg(&grading_path.join(file));
    }
    cp.arg(&working_path);

    match cp.spawn() {
        Err(e) => panic!("Copying grading materials to {:?} failed with {}", working_path, e),
        Ok(mut process) => { process.wait(); Ok(()) }
    }
}

#[derive(Debug, Clone, RustcDecodable, RustcEncodable)]
struct Entry {
    username: String,
    permnum: String,
    full_name: String,
    email: String,
    comments: String,
    grader_output: String,
    score: String,
    letter_grade: String,
    late_days: String
}

impl Entry {
    fn from_readme(readme_path : &Path) -> io::Result<Entry> {
        println!("Constructing Entry from README: {:?}", readme_path);
        let mut readme = try!(OpenOptions::new()
            .read(true)
            .create(true)
            .open(readme_path));

        let mut content = "".to_string();

        try!(readme.read_to_string(&mut content));
        let value = toml::Parser::new(&content[..]).parse();
        match value {
            None => Ok(Entry::parse_error()),
            Some(table) => {
                let empty_string = "error";
                println!("{:?}", table);

                Ok(Entry {
                    username: table.get("username").and_then(|v| v.as_str()).unwrap_or(empty_string).to_string(),
                    permnum: "".to_string(),
                    full_name: table.get("name").and_then(|v| v.as_str()).unwrap_or(empty_string).to_string(),
                    email: table.get("email").and_then(|v| v.as_str()).unwrap_or(empty_string).to_string(),
                    comments: "".to_string(),
                    grader_output: "".to_string(),
                    score: "".to_string(),
                    letter_grade: "".to_string(),
                    late_days: "".to_string()
                })
            }
        }
    }

    fn parse_error() -> Entry {
        Entry {
            username: "error".to_string(),
            permnum: "error".to_string(),
            full_name: "error".to_string(),
            email: "error".to_string(),
            comments: "error".to_string(),
            grader_output: "erorr".to_string(),
            score: "".to_string(),
            letter_grade: "".to_string(),
            late_days: "".to_string()
        }
    }
}

fn main() {
    let args: Args = Args::docopt()
        .decode()
        .unwrap_or_else(|e| e.exit());

    match Grader::new(args).run() {
        Err(e) => println!("{}", e),
        Ok(()) => {}
    }
}

#[derive(Debug)]
struct Grader {
    args: Args
}

impl Grader {
    fn new(args: Args) -> Grader {
        Grader { args: args }
    }

    fn run(&self) -> io::Result<()> {
        let args = self.args.clone();

        let mut user_map = self.load_template();

        let wd = try!(current_dir());

        let submissions: Vec<_> = read_dir(&wd).unwrap().filter_map(|dir_entry| {
            let path = dir_entry.unwrap().path();

            if path.is_dir() {
                Some(path)
            } else {
                None
            }
        }).collect();

        let (tx, rx) = channel();

        let total = submissions.len();

        let pool = ThreadPool::new(4);

        for assignment in submissions {
            // println!("{:?}", assignment);
            let tx = tx.clone();
            let args = args.clone();
            let path = wd.join(&assignment).join("README");

            pool.execute(move || {
                // std::old_io::timer::sleep(Duration::seconds(5));

                in_directory(&Path::new(assignment.clone().into_os_string().to_str().unwrap()), || {
                    let username =
                        clean_username(assignment.file_name().unwrap().to_str().unwrap());

                    println!("Username cleaning {:?} to {}", assignment, username);

                    let required_materials =
                        read_dir(&Path::new(&args.arg_material_path.clone()))
                        .unwrap()
                        .map(|s| PathBuf::from(s.unwrap().path().into_os_string().to_str().unwrap()))
                        .collect();

                    println!("Materials: {:?}", required_materials);

                    copy_grading_materials(&args, required_materials).unwrap();

                    // add timeout support here
                    let stdout = Command::new(args.arg_command.clone())
                        .output().unwrap_or_else(|e| {
                            panic!("Grader failed on: {}", e)
                        }).stdout;

                    let buffered_stdout = BufReader::new(Cursor::new(stdout));
                    let lines: Vec<_> = buffered_stdout.lines().map(|l| l.unwrap()).collect();

                    let mut result = "".to_string();

                    let start = args.flag_n.unwrap_or(lines.len());
                    let output_size = lines.len() - start;

                    for line in &lines[output_size..] {
                        result.push_str(line);
                        result.push_str("\n")
                    }

                    tx.clone().send(Assignment {
                        path: path.clone(),
                        username: username,
                        result: result
                    });
                });
            });
        }

        let mut writer = csv::Writer::from_file(&Path::new("grading.csv")).unwrap();

        for assignment in rx.iter().take(total) {
            println!("Attempting to update {} with {}", assignment.username, assignment.result);

            let mut student = match user_map.remove(&assignment.username) {
                None => {
                    try!(Entry::from_readme(&assignment.path))
                },
                Some(mut e) => e
            };

            student.username = assignment.username;
            student.grader_output = assignment.result;

            writer.encode(student.clone());
            writer.flush();
        }

        for user in user_map.values() {
            writer.encode(user);
        }
        // let mut writer = csv::Writer::from_file(&Path::new("grading.output"));
        //let mut values: Vec<_> = user_map.values().collect();
        //values.sort_by(|a, b| a.username.cmp(&b.username));
        //for entry in values.iter() {
        //writer.encode(entry);
        //}

        println!("At the end of the grader!");
        Ok(())
    }

    fn load_template(&self) -> HashMap<String, Entry> {
        match self.args.flag_t {
            None => HashMap::new(),
            Some(ref template_file) => {
                let mut template = csv::Reader::from_file(&Path::new(&template_file)).unwrap();
                let mut template_map = HashMap::new();

                for record in template.decode() {
                    let entry: Entry = record.unwrap();
                    // println!("{:?}", t);
                    template_map.insert(entry.username.clone(), entry);
                }

                template_map
            }
        }
    }
}

fn clean_username(path: &str) -> String {
    let name = path.split(|c| c == '-').next().unwrap();
    name.to_string()
}

#[derive(Debug)]
struct Assignment {
    path: PathBuf,
    username: String,
    result: String
}

