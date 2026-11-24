class PostsController < ApplicationController
  def index
    @posts = fetch_posts
  end

  private

  def fetch_posts
    Post.all
  end
end
