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
# Network Load Balancer
# ============================================================

resource "aws_lb" "rendezvous" {
  name               = var.name
  load_balancer_type = "network"
  internal           = false
  subnets            = var.subnet_ids

  enable_deletion_protection = false
}

# Target group — TCP port 10000 with HTTP health check on 10001
resource "aws_lb_target_group" "rendezvous" {
  name        = var.name
  port        = 10000
  protocol    = "TCP"
  target_type = "ip"
  vpc_id      = var.vpc_id

  health_check {
    protocol            = "HTTP"
    port                = "10001"
    path                = "/livez"
    healthy_threshold   = 2
    unhealthy_threshold = 2
    interval            = 30
  }

  deregistration_delay = 30
}

resource "aws_lb_listener" "rendezvous" {
  load_balancer_arn = aws_lb.rendezvous.arn
  port              = 10000
  protocol          = "TCP"

  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.rendezvous.arn
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

  load_balancer {
    target_group_arn = aws_lb_target_group.rendezvous.arn
    container_name   = var.name
    container_port   = 10000
  }

  deployment_minimum_healthy_percent = 100
  deployment_maximum_percent         = 200

  depends_on = [aws_lb_listener.rendezvous]
}

# ============================================================
# Route 53
# ============================================================

resource "aws_route53_record" "rendezvous" {
  zone_id = var.route53_zone_id
  name    = var.dns_name
  type    = "A"

  alias {
    name                   = aws_lb.rendezvous.dns_name
    zone_id                = aws_lb.rendezvous.zone_id
    evaluate_target_health = true
  }
}