//! SigV4 against the published AWS examples (credentials `AKIDEXAMPLE`, 2015-08-30T12:36:00Z).

use grauwolf_bedrock::sigv4::{
    Credentials, RequestParts, Scope, amz_date, sign, signing_key, uri_encode,
};

fn example_credentials() -> Credentials {
    Credentials {
        access_key_id: "AKIDEXAMPLE".into(),
        secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
        session_token: None,
    }
}

#[test]
fn derives_the_signing_key_of_the_aws_documentation_example() {
    let key = signing_key(
        "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
        "20150830",
        Scope {
            region: "us-east-1",
            service: "iam",
        },
    );
    let hex: String = key.iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(
        hex,
        "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9"
    );
}

#[test]
fn signs_the_aws_iam_list_users_example() {
    let signed = sign(
        &example_credentials(),
        Scope {
            region: "us-east-1",
            service: "iam",
        },
        &RequestParts {
            method: "GET",
            path: "/",
            query: "Action=ListUsers&Version=2010-05-08",
            headers: &[
                (
                    "Content-Type",
                    "application/x-www-form-urlencoded; charset=utf-8",
                ),
                ("Host", "iam.amazonaws.com"),
                ("X-Amz-Date", "20150830T123600Z"),
            ],
            body: b"",
        },
        "20150830T123600Z",
    );
    assert_eq!(
        signed.signature,
        "5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d7"
    );
    assert_eq!(
        signed.authorization,
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/iam/aws4_request, \
         SignedHeaders=content-type;host;x-amz-date, \
         Signature=5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d7"
    );
}

#[test]
fn signs_the_get_vanilla_test_vector() {
    let signed = sign(
        &example_credentials(),
        Scope {
            region: "us-east-1",
            service: "service",
        },
        &RequestParts {
            method: "GET",
            path: "/",
            query: "",
            headers: &[
                ("Host", "example.amazonaws.com"),
                ("X-Amz-Date", "20150830T123600Z"),
            ],
            body: b"",
        },
        "20150830T123600Z",
    );
    assert_eq!(
        signed.authorization,
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
         SignedHeaders=host;x-amz-date, \
         Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
    );
}

#[test]
fn signs_the_post_vanilla_test_vector() {
    let signed = sign(
        &example_credentials(),
        Scope {
            region: "us-east-1",
            service: "service",
        },
        &RequestParts {
            method: "POST",
            path: "/",
            query: "",
            headers: &[
                ("Host", "example.amazonaws.com"),
                ("X-Amz-Date", "20150830T123600Z"),
            ],
            body: b"",
        },
        "20150830T123600Z",
    );
    assert_eq!(
        signed.signature,
        "5da7c1a2acd57cee7505fc6676e4e544621c30862966e37dddb68e92efbe5d6b"
    );
}

#[test]
fn formats_unix_time_as_the_amz_date() {
    assert_eq!(amz_date(1_440_938_160), "20150830T123600Z");
    assert_eq!(amz_date(0), "19700101T000000Z");
    assert_eq!(amz_date(1_709_164_799), "20240228T235959Z");
    assert_eq!(amz_date(1_709_164_800), "20240229T000000Z");
}

#[test]
fn encodes_everything_but_unreserved_characters() {
    assert_eq!(uri_encode("a b/c:d~e-f_g.h"), "a%20b%2Fc%3Ad~e-f_g.h");
}

#[test]
fn debug_output_hides_the_secrets() {
    let shown = format!("{:?}", example_credentials());
    assert!(
        !shown.contains("wJalr") && !shown.contains("AKID"),
        "{shown}"
    );
}
