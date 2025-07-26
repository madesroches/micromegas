sudo yum update
sudo yum install htop python3-docker docker git -y
sudo usermod -a -G docker ec2-user
id ec2-user
newgrp docker
sudo systemctl enable docker.service
sudo systemctl start docker.service
git clone https://github.com/madesroches/micromegas.git
