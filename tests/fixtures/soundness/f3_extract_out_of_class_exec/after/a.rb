class A
  def run(klass)
    klass.class_exec do
      setup
    end
  end

  def setup
    @a = 1
    @b = 2
    @c = @a + @b
  end
end
