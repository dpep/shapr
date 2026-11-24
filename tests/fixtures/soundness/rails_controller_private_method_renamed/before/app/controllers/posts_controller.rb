class PostsController < ApplicationController
  def index
    @posts = load_posts
  end

  private

  def load_posts
    Post.all
  end
end
