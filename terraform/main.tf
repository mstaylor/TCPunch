# ============================================================
# ECR Repository (existing)
# ============================================================

data "aws_ecr_repository" "rendezvous" {
  name = var.ecr_repository_name
}

# ============================================================
# CloudWatch Log Group
# ============================================================

resource "aws_cloudwatch_log_group" "rendezvous" {
  name              = "/ecs/${var.name}"
  retention_in_days = 30
}

# ============================================================
# IAM — ECS Task Execution Role
# ============================================================

data "aws_iam_policy_document" "ecs_assume_role" {
  statement {
    actions = ["sts:AssumeRole"]
    principals {
      type        = "Service"
      identifiers = ["ecs-tasks.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "ecs_execution" {
  name               = "${var.name}-ecs-execution"
  assume_role_policy = data.aws_iam_policy_document.ecs_assume_role.json
}

resource "aws_iam_role_policy_attachment" "ecs_execution" {
  role       = aws_iam_role.ecs_execution.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}

# IAM — ECS Task Role (used by the running container)
resource "aws_iam_role" "ecs_task" {
  name               = "${var.name}-ecs-task"
  assume_role_policy = data.aws_iam_policy_document.ecs_assume_role.json
}

data "aws_iam_policy_document" "task_permissions" {
  statement {
    sid = "Route53SelfRegister"
    actions = [
      "route53:ChangeResourceRecordSets",
    ]
    resources = ["arn:aws:route53:::hostedzone/${var.route53_zone_id}"]
  }

  statement {
    sid = "DescribeENI"
    actions = [
      "ec2:DescribeNetworkInterfaces",
    ]
    resources = ["*"]
  }
}

resource "aws_iam_role_policy" "ecs_task" {
  name   = "${var.name}-task-permissions"
  role   = aws_iam_role.ecs_task.id
  policy = data.aws_iam_policy_document.task_permissions.json
}

# ============================================================
# ECS Cluster (existing)
# ============================================================

data "aws_ecs_cluster" "rendezvous" {
  cluster_name = var.ecs_cluster_name
}

# ============================================================
# ECS Task Definition
# ============================================================

resource "aws_ecs_task_definition" "rendezvous" {
  family                   = var.name
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = var.task_cpu
  memory                   = var.task_memory
  execution_role_arn       = aws_iam_role.ecs_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  runtime_platform {
    operating_system_family = "LINUX"
    cpu_architecture        = "X86_64"
  }

  container_definitions = jsonencode([
    {
      name      = var.name
      image     = "${data.aws_ecr_repository.rendezvous.repository_url}:${var.image_tag}"
      essential = true

      portMappings = [
        {
          containerPort = 10000
          protocol      = "tcp"
        },
        {
          containerPort = 10001
          protocol      = "tcp"
        }
      ]

      environment = concat(
        [
          { name = "TCPUNCH_PORT", value = "10000" },
          { name = "TCPUNCH_HEALTH_PORT", value = "10001" },
          { name = "TCPUNCH_PEER_TIMEOUT", value = tostring(var.peer_timeout) },
          { name = "TCPUNCH_MAX_CONNECTIONS", value = tostring(var.max_connections) },
          { name = "RUST_LOG", value = var.log_level },
          { name = "ROUTE53_ZONE_ID", value = var.route53_zone_id },
          { name = "ROUTE53_DNS_NAME", value = var.dns_name },
        ],
        var.redis_url != "" ? [{ name = "REDIS_URL", value = var.redis_url }] : []
      )

      logConfiguration = {
        logDriver = "awslogs"
        options = {
          "awslogs-group"         = aws_cloudwatch_log_group.rendezvous.name
          "awslogs-region"        = var.aws_region
          "awslogs-stream-prefix" = "ecs"
        }
      }

      healthCheck = {
        command     = ["CMD-SHELL", "curl -f http://localhost:10001/livez || exit 1"]
        interval    = 30
        timeout     = 5
        retries     = 3
        startPeriod = 10
      }
    }
  ])
}

# ============================================================
# Security Group — ECS Tasks
# ============================================================

resource "aws_security_group" "ecs_tasks" {
  name        = "${var.name}-ecs-tasks"
  description = "Allow inbound from NLB to rendezvous tasks"
  vpc_id      = var.vpc_id

  # NLBs do not have security groups — traffic arrives from the VPC CIDR
  ingress {
    description = "TCP server"
    from_port   = 10000
    to_port     = 10000
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  ingress {
    description = "Health check"
    from_port   = 10001
    to_port     = 10001
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }
}

# ============================================================
# ECS Service
# ============================================================

resource "aws_ecs_service" "rendezvous" {
  name            = var.name
  cluster         = data.aws_ecs_cluster.rendezvous.arn
  task_definition = aws_ecs_task_definition.rendezvous.arn
  desired_count   = var.desired_count
  launch_type     = "FARGATE"

  network_configuration {
    subnets          = var.subnet_ids
    security_groups  = [aws_security_group.ecs_tasks.id]
    assign_public_ip = true
  }

  deployment_minimum_healthy_percent = 100
  deployment_maximum_percent         = 200
}

# Route 53 is managed by the container's entrypoint script.
# On startup, the container registers its public IP in Route 53
# using the ROUTE53_ZONE_ID and ROUTE53_DNS_NAME environment variables.