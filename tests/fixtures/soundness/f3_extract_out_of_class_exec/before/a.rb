class A
  def run(klass)
    klass.class_exec do
      @a = 1
      @b = 2
      @c = @a + @b
    end
  end
end
