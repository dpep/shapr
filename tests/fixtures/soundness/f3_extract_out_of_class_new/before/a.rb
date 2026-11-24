class A
  def build
    Class.new do
      @a = 1
      @b = 2
      @c = @a + @b
    end
  end
end
