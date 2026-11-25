RSpec.describe Config do
  it "opts in" do
    model = stub_const("OptedIn", Class.new(Base) { polyid })
    expect(model.attribute).to eq("uuid")
  end
end
